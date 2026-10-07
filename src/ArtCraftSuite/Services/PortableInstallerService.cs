using System.Diagnostics;
using System.IO.Compression;
using System.Text.Json;
using ArtCraftSuite.Models;

namespace ArtCraftSuite.Services;

public sealed class PortableInstallerService
{
    private readonly string _root = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "ArtCraftSuite");
    private string StatePath => Path.Combine(_root, "state.json");
    private string AppsRoot => Path.Combine(_root, "apps");
    private static readonly JsonSerializerOptions JsonOptions = new() { WriteIndented = true, PropertyNameCaseInsensitive = true };

    public InstallState LoadState()
    {
        try
        {
            if (File.Exists(StatePath)) return JsonSerializer.Deserialize<InstallState>(File.ReadAllText(StatePath), JsonOptions) ?? new();
        }
        catch (JsonException) { }
        return new();
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
        var backup = target + ".previous";
        Directory.CreateDirectory(staging);
        try
        {
            await Task.Run(() => ExtractSafely(archive, staging, ct), ct);
            var executable = Directory.EnumerateFiles(staging, "*.exe", SearchOption.AllDirectories)
                .OrderBy(x => x.Count(c => c == Path.DirectorySeparatorChar))
                .FirstOrDefault(x => Path.GetFileNameWithoutExtension(x).Equals(app.Id, StringComparison.OrdinalIgnoreCase))
                ?? throw new InvalidDataException($"The verified archive did not contain {app.Id}.exe.");
            var relativeExecutable = Path.GetRelativePath(staging, executable);
            if (Directory.Exists(backup)) Directory.Delete(backup, true);
            if (Directory.Exists(target)) Directory.Move(target, backup);
            try { Directory.Move(staging, target); }
            catch { if (Directory.Exists(backup)) Directory.Move(backup, target); throw; }
            if (Directory.Exists(backup)) Directory.Delete(backup, true);

            var installed = new InstalledApp(app.Id, release.Version, Path.Combine(target, relativeExecutable), release.Asset.Name, release.Sha256, DateTimeOffset.UtcNow);
            var state = LoadState();
            state.Apps[app.Id] = installed;
            SaveState(state);
            return installed;
        }
        finally { if (Directory.Exists(staging)) Directory.Delete(staging, true); }
    }

    public void Uninstall(string id)
    {
        var state = LoadState();
        if (state.Apps.TryGetValue(id, out var installed))
        {
            var appDir = Path.GetFullPath(Path.Combine(AppsRoot, id));
            var safeRoot = Path.GetFullPath(AppsRoot) + Path.DirectorySeparatorChar;
            if (!appDir.StartsWith(safeRoot, StringComparison.OrdinalIgnoreCase)) throw new InvalidOperationException("Unsafe uninstall path.");
            if (Directory.Exists(appDir)) Directory.Delete(appDir, true);
            state.Apps.Remove(id);
            SaveState(state);
        }
    }

    public static void Launch(InstalledApp app)
    {
        if (!File.Exists(app.ExecutablePath)) throw new FileNotFoundException("The installed application executable is missing.", app.ExecutablePath);
        Process.Start(new ProcessStartInfo(app.ExecutablePath) { UseShellExecute = true, WorkingDirectory = Path.GetDirectoryName(app.ExecutablePath) });
    }

    private void SaveState(InstallState state)
    {
        Directory.CreateDirectory(_root);
        var temp = StatePath + ".tmp";
        File.WriteAllText(temp, JsonSerializer.Serialize(state, JsonOptions));
        File.Move(temp, StatePath, true);
    }

    private static void ExtractSafely(string archive, string destination, CancellationToken ct)
    {
        var root = Path.GetFullPath(destination) + Path.DirectorySeparatorChar;
        using var zip = ZipFile.OpenRead(archive);
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
}
