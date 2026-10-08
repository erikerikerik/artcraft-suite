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
    private readonly Action<string>? _testHook;
    private string StatePath => Path.Combine(_root, "state.json");
    private string AppsRoot => Path.Combine(_root, "apps");
    private string TransactionsRoot => Path.Combine(_root, "transactions");
    private static readonly JsonSerializerOptions JsonOptions = new() { WriteIndented = true, PropertyNameCaseInsensitive = true };

    public PortableInstallerService(string? root = null)
    {
        _root = root ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "ArtCraftSuite");
    }

    internal PortableInstallerService(string root, Action<string>? testHook)
    {
        _root = root;
        _testHook = testHook;
    }

    public InstallState LoadState()
    {
        RecoverInterruptedTransactions();
        if (File.Exists(StatePath))
        {
            var json = ReadWithRetry(StatePath);
            try
            {
                var (state, migrated) = DeserializeState(json);
                if (migrated)
                {
                    try { SaveState(state); }
                    catch (IOException) { }
                    catch (UnauthorizedAccessException) { }
                }
                return state;
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
            throw new PlatformNotSupportedException("Windows ZIP installation is not available on this platform.");
        if (!release.Asset.Name.EndsWith(".zip", StringComparison.OrdinalIgnoreCase))
            throw new InvalidOperationException("The Windows installer expects an official portable ZIP asset.");

        Directory.CreateDirectory(AppsRoot);
        var target = Path.Combine(AppsRoot, app.Id);
        var staging = Path.Combine(AppsRoot, $".{app.Id}-staging-{Guid.NewGuid():N}");
        var backup = Path.Combine(AppsRoot, $".{app.Id}-previous");
        var journalPath = Path.Combine(TransactionsRoot, $"{app.Id}.json");
        Directory.CreateDirectory(staging);
        try
        {
            await Task.Run(() => ExtractSafely(archive, staging, ct), ct);
            var executable = FindExecutable(staging, app.EffectivePackageId, app.Id)
                ?? throw new InvalidDataException($"The verified archive did not contain {app.EffectivePackageId}.exe.");
            var relativeExecutable = Path.GetRelativePath(staging, executable);
            var state = LoadState();
            state.Apps.TryGetValue(app.Id, out var previousInstalled);
            var installed = new InstalledApp(app.Id, release.Version, Path.Combine(target, relativeExecutable), release.Asset.Name, release.Sha256, DateTimeOffset.UtcNow);

            ct.ThrowIfCancellationRequested();
            ThrowIfRunning(app.Name, target);
            if (Directory.Exists(backup)) Directory.Delete(backup, true);
            var journal = new InstallJournal(app.Id, target, staging, backup, TransactionPhase.Prepared, previousInstalled, installed);
            SaveJournal(journalPath, journal);

            if (Directory.Exists(target))
            {
                try { Directory.Move(target, backup); }
                catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
                {
                    throw new InvalidOperationException($"{app.Name} appears to be open or its folder is in use. Close it and try again.", ex);
                }
            }
            journal = journal with { Phase = TransactionPhase.BackedUp };
            SaveJournal(journalPath, journal);

            try
            {
                Directory.Move(staging, target);
                journal = journal with { Phase = TransactionPhase.Activated };
                SaveJournal(journalPath, journal);
                _testHook?.Invoke("AfterActivate");
                state.Apps[app.Id] = installed;
                SaveState(state);
                SaveJournal(journalPath, journal with { Phase = TransactionPhase.StateSaved });
            }
            catch (Exception installError)
            {
                try
                {
                    MoveAsideAndDelete(target, app.Id);
                    if (Directory.Exists(backup)) Directory.Move(backup, target);
                    RestorePreviousState(app.Id, previousInstalled);
                    TryDeleteFile(journalPath);
                }
                catch (Exception rollbackError)
                {
                    throw new AggregateException($"{app.Name} installation failed and the previous version could not be restored.", installError, rollbackError);
                }
                throw;
            }

            TryDeleteFile(journalPath);
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
            ThrowIfRunning(name, appDir);
            removing = Path.Combine(AppsRoot, $".{id}-removing-{Guid.NewGuid():N}");
            try { Directory.Move(appDir, removing); }
            catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
            {
                throw new InvalidOperationException($"{name} appears to be open or its folder is in use. Close it and try again.", ex);
            }
        }

        try { if (state.Apps.Remove(id)) SaveState(state); }
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

        var previous = Path.Combine(AppsRoot, $".{id}-previous");
        if (removing is null)
        {
            if (Directory.Exists(previous)) TryDelete(previous);
            return null;
        }
        try { Directory.Delete(removing, true); }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            return $"{name} was removed, but old files remain at {removing}. Close the app and delete that folder later.";
        }
        if (Directory.Exists(previous)) TryDelete(previous);
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
            if (id.StartsWith('.')) continue;
            var exe = FindExecutable(dir, id) ?? Directory.EnumerateFiles(dir, "*.exe", SearchOption.AllDirectories)
                .OrderBy(x => x.Count(c => c == Path.DirectorySeparatorChar)).ThenBy(x => x, StringComparer.OrdinalIgnoreCase).FirstOrDefault();
            if (exe is not null)
                state.Apps[id] = new InstalledApp(id, DetectVersion(exe), exe, string.Empty, string.Empty, Directory.GetLastWriteTimeUtc(dir));
        }
        return state;
    }

    private static void ThrowIfRunning(string name, string appDir)
    {
        var root = Path.GetFullPath(appDir) + Path.DirectorySeparatorChar;
        foreach (var process in Process.GetProcesses())
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

    private void RecoverInterruptedTransactions()
    {
        if (!Directory.Exists(TransactionsRoot)) return;
        foreach (var path in Directory.EnumerateFiles(TransactionsRoot, "*.json"))
        {
            InstallJournal journal;
            try { journal = JsonSerializer.Deserialize<InstallJournal>(File.ReadAllText(path), JsonOptions) ?? throw new JsonException("Empty journal."); }
            catch (Exception ex) when (ex is IOException or JsonException)
            {
                throw new IOException($"Interrupted installation recovery data is unreadable: {path}", ex);
            }

            if (journal.Phase is TransactionPhase.Activated
                || journal.Phase is TransactionPhase.BackedUp && Directory.Exists(journal.Target))
            {
                MoveAsideAndDelete(journal.Target, journal.Id);
                if (Directory.Exists(journal.Backup)) Directory.Move(journal.Backup, journal.Target);
                RestorePreviousState(journal.Id, journal.Previous);
            }
            else if (journal.Phase is TransactionPhase.BackedUp && Directory.Exists(journal.Backup))
            {
                Directory.Move(journal.Backup, journal.Target);
            }
            else if (journal.Phase is TransactionPhase.Prepared && !Directory.Exists(journal.Target) && Directory.Exists(journal.Backup))
            {
                Directory.Move(journal.Backup, journal.Target);
            }
            if (Directory.Exists(journal.Staging)) TryDelete(journal.Staging);
            TryDeleteFile(path);
        }
    }

    private void RestorePreviousState(string id, InstalledApp? previous)
    {
        var state = ReadStateWithoutRecovery();
        if (previous is null) state.Apps.Remove(id); else state.Apps[id] = previous;
        SaveState(state);
    }

    private InstallState ReadStateWithoutRecovery()
    {
        if (!File.Exists(StatePath)) return new InstallState();
        try { return DeserializeState(ReadWithRetry(StatePath)).State; }
        catch (JsonException) { return new InstallState(); }
    }

    private static (InstallState State, bool Migrated) DeserializeState(string json)
    {
        using var document = JsonDocument.Parse(json);
        if (document.RootElement.ValueKind != JsonValueKind.Object)
            throw new JsonException("The install state root must be an object.");

        var state = new InstallState();
        if (!TryGetProperty(document.RootElement, "Apps", out var apps)) return (state, false);
        if (apps.ValueKind != JsonValueKind.Object) throw new JsonException("The install state apps value must be an object.");

        var migrated = !document.RootElement.TryGetProperty("Apps", out _);
        foreach (var entry in apps.EnumerateObject())
        {
            if (entry.Value.ValueKind != JsonValueKind.Object) throw new JsonException($"Install state entry '{entry.Name}' must be an object.");
            var item = entry.Value;
            var id = ReadString(item, "Id", "id") ?? entry.Name;
            var version = ReadString(item, "Version", "version") ?? UnknownVersion;
            var executable = ReadString(item, "ExecutablePath", "executablePath", "launchPath", "launch_path");
            if (string.IsNullOrWhiteSpace(executable)) throw new JsonException($"Install state entry '{entry.Name}' has no executable path.");

            var sourceAsset = ReadString(item, "SourceAsset", "sourceAsset", "source_asset") ?? string.Empty;
            var sha256 = ReadString(item, "Sha256", "sha256") ?? string.Empty;
            var installedAt = ReadInstalledAt(item);
            if (!item.TryGetProperty("Id", out _) || !item.TryGetProperty("ExecutablePath", out _) || !item.TryGetProperty("InstalledAt", out _))
                migrated = true;
            state.Apps[id] = new InstalledApp(id, version, executable, sourceAsset, sha256, installedAt);
        }
        return (state, migrated);
    }

    private static DateTimeOffset ReadInstalledAt(JsonElement item)
    {
        if (TryGetProperty(item, "InstalledAt", out var installedAt) && installedAt.ValueKind == JsonValueKind.String
            && installedAt.TryGetDateTimeOffset(out var timestamp)) return timestamp;
        if (TryGetProperty(item, "installedAtUnix", out var unix) && unix.TryGetInt64(out var seconds))
        {
            try { return DateTimeOffset.FromUnixTimeSeconds(seconds); }
            catch (ArgumentOutOfRangeException ex) { throw new JsonException("The legacy install timestamp is out of range.", ex); }
        }
        return DateTimeOffset.UnixEpoch;
    }

    private static string? ReadString(JsonElement element, params string[] names)
    {
        foreach (var name in names)
            if (TryGetProperty(element, name, out var value) && value.ValueKind == JsonValueKind.String)
                return value.GetString();
        return null;
    }

    private static bool TryGetProperty(JsonElement element, string name, out JsonElement value)
    {
        foreach (var property in element.EnumerateObject())
        {
            if (!property.Name.Equals(name, StringComparison.OrdinalIgnoreCase)) continue;
            value = property.Value;
            return true;
        }
        value = default;
        return false;
    }

    private void SaveJournal(string path, InstallJournal journal)
    {
        Directory.CreateDirectory(TransactionsRoot);
        AtomicWrite(path, JsonSerializer.Serialize(journal, JsonOptions));
    }

    private void SaveState(InstallState state)
    {
        Directory.CreateDirectory(_root);
        AtomicWrite(StatePath, JsonSerializer.Serialize(state, JsonOptions));
    }

    private static void AtomicWrite(string path, string contents)
    {
        var temp = path + $".{Guid.NewGuid():N}.tmp";
        try { File.WriteAllText(temp, contents); File.Move(temp, path, true); }
        finally { TryDeleteFile(temp); }
    }

    private static string ReadWithRetry(string path)
    {
        for (var attempt = 1; ; attempt++)
        {
            try { return File.ReadAllText(path); }
            catch (IOException) when (attempt < 5) { Thread.Sleep(100 * attempt); }
        }
    }

    private static void MoveAsideAndDelete(string target, string id)
    {
        if (!Directory.Exists(target)) return;
        var failed = Path.Combine(Path.GetDirectoryName(target)!, $".{id}-failed-{Guid.NewGuid():N}");
        Directory.Move(target, failed);
        TryDelete(failed);
    }

    private static void TryDelete(string directory)
    {
        try { Directory.Delete(directory, true); }
        catch (IOException) { }
        catch (UnauthorizedAccessException) { }
    }

    private static void TryDeleteFile(string path)
    {
        try { File.Delete(path); }
        catch (IOException) { }
        catch (UnauthorizedAccessException) { }
    }

    internal static void ExtractSafely(string archive, string destination, CancellationToken ct)
    {
        var root = Path.GetFullPath(destination) + Path.DirectorySeparatorChar;
        using var zip = ZipFile.OpenRead(archive);
        if (zip.Entries.Count > MaxArchiveEntries) throw new InvalidDataException($"Archive contains too many entries ({zip.Entries.Count:N0}).");
        long totalLength = 0;
        foreach (var entry in zip.Entries)
        {
            if (((entry.ExternalAttributes >> 16) & 0xF000) == 0xA000) throw new InvalidDataException("Archive contains a symbolic link, which is not allowed.");
            try { totalLength = checked(totalLength + entry.Length); }
            catch (OverflowException) { throw new InvalidDataException("Archive declares an invalid extracted size."); }
            if (totalLength > MaxExtractedBytes) throw new InvalidDataException($"Archive expands beyond the {MaxExtractedBytes / (1024 * 1024 * 1024)} GiB safety limit.");
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
            .OrderBy(x => x.Count(c => c == Path.DirectorySeparatorChar)).ThenBy(x => x, StringComparer.OrdinalIgnoreCase).FirstOrDefault();
    }

    private static string DetectVersion(string executable)
    {
        try
        {
            var candidate = FileVersionInfo.GetVersionInfo(executable).ProductVersion ?? FileVersionInfo.GetVersionInfo(executable).FileVersion;
            return string.IsNullOrWhiteSpace(candidate) ? UnknownVersion : candidate.Split('+')[0];
        }
        catch { return UnknownVersion; }
    }

    private enum TransactionPhase { Prepared, BackedUp, Activated, StateSaved }
    private sealed record InstallJournal(string Id, string Target, string Staging, string Backup, TransactionPhase Phase, InstalledApp? Previous, InstalledApp Next);
}
