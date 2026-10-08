using System.Diagnostics;
using System.IO.Compression;
using System.Text.Json;
using ArtCraftSuite.Models;

namespace ArtCraftSuite.Services;

public sealed class PortableInstallerService
{
    public const string UnknownVersion = "unknown";
    internal const long MaxExtractedBytes = 16L * 1024 * 1024 * 1024;
    internal const int MaxArchiveEntries = 100_000;
    private readonly string _root;
    private string StatePath => Path.Combine(_root, "state.json");
    private string AppsRoot => Path.Combine(_root, "apps");
    private static readonly JsonSerializerOptions JsonOptions = new() { WriteIndented = true, PropertyNameCaseInsensitive = true };

    public PortableInstallerService(string? root = null)
    {
        _root = root ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "ArtCraftSuite");
    }

    /// <summary>
    /// Loads install state. If state.json is missing or cannot be parsed, a damaged file is kept aside and
    /// the list is rebuilt from the manager-owned app folders, so one bad file never makes the manager
    /// forget (and later overwrite the record of) every installed app.
    /// </summary>
    public InstallState LoadState()
    {
        if (File.Exists(StatePath))
        {
            // A read error (for example antivirus briefly holding the file) is retried and then surfaced;
            // only content that fails to parse is treated as damaged.
            var json = ReadWithRetry(StatePath);
            try
            {
                var state = JsonSerializer.Deserialize<InstallState>(json, JsonOptions);
                if (state is not null) return state;
            }
            catch (JsonException) { }

            var damagedPath = $"{StatePath}.corrupt-{Guid.NewGuid():N}";
            try { File.Move(StatePath, damagedPath); }
            catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
            {
                throw new IOException("The damaged state file could not be preserved. Close other programs using it and try again.", ex);
            }
        }

        var rebuilt = RebuildStateFromDisk();
        if (rebuilt.Apps.Count > 0)
        {
            try { SaveState(rebuilt); }
            catch (IOException) { }
            catch (UnauthorizedAccessException) { }
        }
        return rebuilt;
    }

    public async Task<InstalledApp> InstallZipAsync(AppManifest app, ResolvedRelease release, string archive, CancellationToken ct)
    {
        if (!OperatingSystem.IsWindows())
            throw new PlatformNotSupportedException("macOS ARM support is scaffolded for builds; DMG installation will be enabled in a later release.");
        if (!release.Asset.Name.EndsWith(".zip", StringComparison.OrdinalIgnoreCase))
            throw new InvalidOperationException("The Windows installer expects an official portable ZIP asset.");

        Directory.CreateDirectory(AppsRoot);
        var target = Path.Combine(AppsRoot, app.Id);
        var staging = Path.Combine(AppsRoot, $".{app.Id}-staging-{Guid.NewGuid():N}");
        var backup = Path.Combine(AppsRoot, $".{app.Id}-previous-{Guid.NewGuid():N}");
        Directory.CreateDirectory(staging);
        try
        {
            await Task.Run(() => ExtractSafely(archive, staging, ct), ct);
            var executable = FindExecutable(staging, app.EffectivePackageId, app.Id)
                ?? throw new InvalidDataException($"The verified archive did not contain {app.EffectivePackageId}.exe.");
            var relativeExecutable = Path.GetRelativePath(staging, executable);
            var state = LoadState();
            var installed = new InstalledApp(app.Id, release.Version, Path.Combine(target, relativeExecutable), release.Asset.Name, release.Sha256, DateTimeOffset.UtcNow);

            // Past this point the swap must not be interrupted half-way.
            ct.ThrowIfCancellationRequested();
            ThrowIfRunning(app.Id, app.Name, target);
            if (Directory.Exists(target))
            {
                try { Directory.Move(target, backup); }
                catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
                {
                    throw new InvalidOperationException($"{app.Name} appears to be open or its folder is in use. Close it and try again.", ex);
                }
            }
            try
            {
                Directory.Move(staging, target);
                state.Apps[app.Id] = installed;
                SaveState(state);
            }
            catch (Exception installError)
            {
                try
                {
                    if (Directory.Exists(target))
                    {
                        var failed = Path.Combine(AppsRoot, $".{app.Id}-failed-{Guid.NewGuid():N}");
                        Directory.Move(target, failed);
                        TryDelete(failed);
                    }
                    if (Directory.Exists(backup)) Directory.Move(backup, target);
                }
                catch (Exception rollbackError)
                {
                    throw new AggregateException($"{app.Name} installation failed and the previous version could not be restored.", installError, rollbackError);
                }
                throw;
            }
            if (Directory.Exists(backup)) TryDelete(backup);
            return installed;
        }
        finally { if (Directory.Exists(staging)) TryDelete(staging); }
    }

    public string? Uninstall(string id, string name)
    {
        var state = LoadState();
        var appDir = Path.GetFullPath(Path.Combine(AppsRoot, id));
        var safeRoot = Path.GetFullPath(AppsRoot) + Path.DirectorySeparatorChar;
        if (!appDir.StartsWith(safeRoot, StringComparison.OrdinalIgnoreCase)) throw new InvalidOperationException("Unsafe uninstall path.");

        string? removing = null;
        if (Directory.Exists(appDir))
        {
            ThrowIfRunning(id, name, appDir);
            removing = Path.Combine(AppsRoot, $".{id}-removing-{Guid.NewGuid():N}");
            try { Directory.Move(appDir, removing); }
            catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
            {
                throw new InvalidOperationException($"{name} appears to be open or its folder is in use. Close it and try again.", ex);
            }
        }

        try
        {
            if (state.Apps.Remove(id)) SaveState(state);
        }
        catch (Exception stateError)
        {
            if (removing is not null)
            {
                try { Directory.Move(removing, appDir); }
                catch (Exception rollbackError)
                {
                    throw new AggregateException($"{name} removal failed and its folder could not be restored.", stateError, rollbackError);
                }
            }
            throw;
        }

        if (removing is null) return null;
        try { Directory.Delete(removing, true); }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            return $"{name} was removed, but some old files remain at {removing}. Close the app and delete that folder when it is no longer in use.";
        }
        return null;
    }

    public static void Launch(InstalledApp app)
    {
        if (!File.Exists(app.ExecutablePath)) throw new FileNotFoundException("The installed application executable is missing.", app.ExecutablePath);
        Process.Start(new ProcessStartInfo(app.ExecutablePath) { UseShellExecute = true, WorkingDirectory = Path.GetDirectoryName(app.ExecutablePath) });
    }

    private InstallState RebuildStateFromDisk()
    {
        var state = new InstallState();
        if (!Directory.Exists(AppsRoot)) return state;
        foreach (var dir in Directory.EnumerateDirectories(AppsRoot))
        {
            var id = Path.GetFileName(dir);
            if (id.StartsWith('.') || id.EndsWith(".previous", StringComparison.OrdinalIgnoreCase)) continue;
            var exe = FindExecutable(dir, id) ?? Directory.EnumerateFiles(dir, "*.exe", SearchOption.AllDirectories)
                .OrderBy(x => x.Count(c => c == Path.DirectorySeparatorChar))
                .ThenBy(x => x, StringComparer.OrdinalIgnoreCase)
                .FirstOrDefault();
            if (exe is null) continue;
            state.Apps[id] = new InstalledApp(id, DetectVersion(exe), exe, string.Empty, string.Empty, Directory.GetLastWriteTimeUtc(dir));
        }
        return state;
    }

    private static void ThrowIfRunning(string id, string name, string appDir)
    {
        var root = Path.GetFullPath(appDir) + Path.DirectorySeparatorChar;
        foreach (var process in Process.GetProcessesByName(id))
        {
            using (process)
            {
                string? path;
                try { path = process.MainModule?.FileName; }
                catch { continue; }
                if (path is not null && Path.GetFullPath(path).StartsWith(root, StringComparison.OrdinalIgnoreCase))
                    throw new InvalidOperationException($"{name} is open. Close it and try again.");
            }
        }
    }

    private static string ReadWithRetry(string path)
    {
        for (var attempt = 1; ; attempt++)
        {
            try { return File.ReadAllText(path); }
            catch (IOException) when (attempt < 5) { Thread.Sleep(100 * attempt); }
        }
    }

    private static void TryDelete(string directory)
    {
        try { Directory.Delete(directory, true); }
        catch (IOException) { }
        catch (UnauthorizedAccessException) { }
    }

    private void SaveState(InstallState state)
    {
        Directory.CreateDirectory(_root);
        var temp = StatePath + ".tmp";
        try
        {
            File.WriteAllText(temp, JsonSerializer.Serialize(state, JsonOptions));
            File.Move(temp, StatePath, true);
        }
        finally
        {
            try { if (File.Exists(temp)) File.Delete(temp); } catch { }
        }
    }

    internal static void ExtractSafely(string archive, string destination, CancellationToken ct)
    {
        var root = Path.GetFullPath(destination) + Path.DirectorySeparatorChar;
        using var zip = ZipFile.OpenRead(archive);
        if (zip.Entries.Count > MaxArchiveEntries)
            throw new InvalidDataException($"Archive contains too many entries ({zip.Entries.Count:N0}).");
        long totalLength = 0;
        foreach (var entry in zip.Entries)
        {
            if (((entry.ExternalAttributes >> 16) & 0xF000) == 0xA000)
                throw new InvalidDataException("Archive contains a symbolic link, which is not allowed.");
            try { totalLength = checked(totalLength + entry.Length); }
            catch (OverflowException) { throw new InvalidDataException("Archive declares an invalid extracted size."); }
            if (totalLength > MaxExtractedBytes)
                throw new InvalidDataException($"Archive expands beyond the {MaxExtractedBytes / (1024 * 1024 * 1024)} GiB safety limit.");
        }
        foreach (var entry in zip.Entries)
        {
            ct.ThrowIfCancellationRequested();
            var path = Path.GetFullPath(Path.Combine(destination, entry.FullName));
            if (!path.StartsWith(root, StringComparison.OrdinalIgnoreCase)) throw new InvalidDataException("Archive contains an unsafe path.");
            if (string.IsNullOrEmpty(entry.Name)) { Directory.CreateDirectory(path); continue; }
            Directory.CreateDirectory(Path.GetDirectoryName(path)!);
            entry.ExtractToFile(path, true);
        }
    }

    private static string? FindExecutable(string directory, params string[] acceptedStems)
    {
        var accepted = acceptedStems.ToHashSet(StringComparer.OrdinalIgnoreCase);
        return Directory.EnumerateFiles(directory, "*.exe", SearchOption.AllDirectories)
            .Where(x => accepted.Contains(Path.GetFileNameWithoutExtension(x)))
            .OrderBy(x => x.Count(c => c == Path.DirectorySeparatorChar))
            .ThenBy(x => x, StringComparer.OrdinalIgnoreCase)
            .FirstOrDefault();
    }

    private static string DetectVersion(string executable)
    {
        try
        {
            var info = FileVersionInfo.GetVersionInfo(executable);
            var candidate = info.ProductVersion ?? info.FileVersion;
            return string.IsNullOrWhiteSpace(candidate) ? UnknownVersion : candidate.Split('+')[0];
        }
        catch { return UnknownVersion; }
    }
}
