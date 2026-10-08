using System.Collections.ObjectModel;
using Avalonia.Media.Imaging;
using Avalonia.Platform;
using ArtCraftSuite.Models;
using ArtCraftSuite.Services;

namespace ArtCraftSuite.ViewModels;

public sealed class MainViewModel : ObservableObject
{
    private readonly GitHubReleaseService _releases;
    private readonly PortableInstallerService _installer;
    private readonly DiagnosticService _diagnostics;
    private readonly Func<string, Task>? _copyText;
    private CancellationTokenSource? _operation;
    private bool _isBusy;
    private double _progress;
    private string _status = "Ready. Choose apps, then install.";
    private string _channel = "Stable";

    public ObservableCollection<AppItemViewModel> Apps { get; } = [];
    public IReadOnlyList<string> Channels { get; } = ["Stable", "Latest"];
    public AsyncCommand RefreshCommand { get; }
    public AsyncCommand InstallSelectedCommand { get; }
    public RelayCommand CancelCommand { get; }
    public AsyncCommand CopyDiagnosticsCommand { get; }

    public bool IsBusy
    {
        get => _isBusy;
        private set
        {
            if (!Set(ref _isBusy, value)) return;
            RefreshCommand.Notify(); InstallSelectedCommand.Notify(); CancelCommand.Notify();
            foreach (var app in Apps) app.NotifyCommands();
        }
    }

    public double Progress { get => _progress; private set => Set(ref _progress, value); }
    public string Status
    {
        get => _status;
        private set { if (Set(ref _status, value)) _diagnostics.Info("status", value); }
    }

    public string Channel
    {
        get => _channel;
        set
        {
            // The channel picker is disabled while busy; this guard keeps the cards and the selected
            // channel from ever disagreeing if a change slips through anyway.
            if (IsBusy || !Set(ref _channel, value)) { Raise(); return; }
            _ = RefreshAsync(forceRefresh: false);
        }
    }

    public MainViewModel(SuiteManifest manifest, GitHubReleaseService releases, PortableInstallerService installer,
        DiagnosticService? diagnostics = null, Func<string, Task>? copyText = null)
    {
        _releases = releases;
        _installer = installer;
        _diagnostics = diagnostics ?? new DiagnosticService();
        _copyText = copyText;
        RefreshCommand = new(() => RefreshAsync(forceRefresh: true), () => !IsBusy);
        InstallSelectedCommand = new(InstallSelectedAsync, () => !IsBusy && Apps.Any(x => x.IsSelected && x.Release is not null));
        CancelCommand = new(() => _operation?.Cancel(), () => IsBusy);
        CopyDiagnosticsCommand = new(CopyDiagnosticsAsync);

        var state = installer.LoadState();
        foreach (var item in manifest.Apps)
        {
            state.Apps.TryGetValue(item.Id, out var installed);
            var vm = new AppItemViewModel(item, installed, () => IsBusy, InstallOneAsync, RemoveOneAsync, OpenOne);
            vm.PropertyChanged += (_, e) =>
            {
                if (e.PropertyName is nameof(AppItemViewModel.IsSelected) or nameof(AppItemViewModel.Release)) InstallSelectedCommand.Notify();
            };
            Apps.Add(vm);
        }
        _ = RefreshAsync(forceRefresh: false);
    }

    private async Task CopyDiagnosticsAsync()
    {
        try
        {
            var lines = Apps.Select(app => $"{app.Name}: installed={app.Installed?.Version ?? "no"}, available={app.Release?.Version ?? "unknown"}, error={app.Error ?? "none"}");
            var report = _diagnostics.BuildReport(Channel, Status, lines);
            if (_copyText is null) { Status = $"Diagnostic report is available at {_diagnostics.LogPath}."; return; }
            await _copyText(report);
            Status = "Diagnostic report copied to the clipboard.";
        }
        catch (Exception ex) { _diagnostics.Error("diagnostics.copy", ex); Status = $"Could not copy diagnostics: {ex.Message}"; }
    }

    private string Platform => OperatingSystem.IsWindows() ? "windows-x64" : OperatingSystem.IsMacOS() && System.Runtime.InteropServices.RuntimeInformation.OSArchitecture == System.Runtime.InteropServices.Architecture.Arm64 ? "macos-arm64" : "unsupported";

    private CancellationToken Begin()
    {
        _operation?.Dispose();
        _operation = new CancellationTokenSource();
        Progress = 0;
        IsBusy = true;
        return _operation.Token;
    }

    private void End()
    {
        Progress = 0;
        IsBusy = false;
    }

    private async Task RefreshAsync(bool forceRefresh)
    {
        if (IsBusy) return;
        var ct = Begin();
        Status = $"Checking {Channel.ToLowerInvariant()} releases…";
        try
        {
            var completed = 0;
            foreach (var app in Apps)
            {
                ct.ThrowIfCancellationRequested();
                try { app.Release = await _releases.ResolveAsync(app.Manifest, Channel, Platform, forceRefresh, ct); app.Error = null; }
                catch (OperationCanceledException) { throw; }
                catch (Exception ex) { _diagnostics.Error($"refresh.{app.Manifest.Id}", ex); app.Release = null; app.Error = ex.Message; }
                Progress = ++completed * 100d / Apps.Count;
            }
            var available = Apps.Count(x => x.Release is not null);
            var rateLimit = Apps.Select(x => x.Error).FirstOrDefault(x => x?.Contains("hourly limit", StringComparison.Ordinal) == true);
            Status = available == Apps.Count ? $"All {available} apps are ready."
                : rateLimit ?? $"{available} of {Apps.Count} releases are available. Hover unavailable items for details.";
        }
        catch (OperationCanceledException) { Status = "Check cancelled."; }
        catch (Exception ex) { _diagnostics.Error("refresh", ex); Status = $"Refresh failed: {ex.Message}"; }
        finally { End(); }
    }

    /// <summary>
    /// Installs every selected app as one operation: the manager stays busy for the whole batch, Cancel
    /// stops the batch, and failures are reported at the end instead of being overwritten by later apps.
    /// </summary>
    private async Task InstallSelectedAsync()
    {
        if (IsBusy) return;
        var selected = Apps.Where(x => x.IsSelected).ToList();
        if (selected.Count == 0) return;
        var ct = Begin();
        var installed = 0;
        string? firstFailure = null;
        var failures = 0;
        try
        {
            foreach (var app in selected)
            {
                var error = await InstallCoreAsync(app, ct);
                if (error is null) installed++;
                else { failures++; firstFailure ??= $"{app.Name}: {error}"; }
            }
            if (firstFailure is null)
                Status = selected.Count == 1 ? Status : $"All {installed} selected apps are installed.";
            else
                Status = $"Installed {installed} of {selected.Count}. Could not install {firstFailure}"
                    + (failures > 1 ? $" (and {failures - 1} more)" : string.Empty);
        }
        catch (OperationCanceledException)
        {
            Status = $"Install cancelled after {installed} of {selected.Count} apps. The app in progress was not changed.";
        }
        finally { End(); }
    }

    private async Task InstallOneAsync(AppItemViewModel app)
    {
        if (IsBusy) return;
        var ct = Begin();
        try
        {
            var error = await InstallCoreAsync(app, ct);
            if (error is not null) Status = $"Could not install {app.Name}: {error}";
        }
        catch (OperationCanceledException) { Status = $"{app.Name} install cancelled. Nothing was changed."; }
        finally { End(); }
    }

    /// <returns>Null on success, otherwise the error message. Cancellation is thrown, not returned.</returns>
    private async Task<string?> InstallCoreAsync(AppItemViewModel app, CancellationToken ct)
    {
        string? temp = null;
        var installedSuccessfully = false;
        try
        {
            // Always resolve against the channel currently shown, so a release from a previously selected
            // channel can never be installed. This is served from cache and costs no extra API call.
            app.Release = await _releases.ResolveAsync(app.Manifest, Channel, Platform, forceRefresh: false, ct);
            app.Error = null;
            var downloads = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "ArtCraftSuite", "downloads");
            Directory.CreateDirectory(downloads);
            temp = Path.Combine(downloads, $"{app.Manifest.Id}-{app.Release.Sha256}.partial");
            Progress = 0;
            Status = File.Exists(temp)
                ? $"Resuming and verifying {app.Name} {app.Release.Version}…"
                : $"Downloading {app.Name} {app.Release.Version}…";
            await _releases.DownloadVerifiedAsync(app.Release, temp, new Progress<double>(x => Progress = x), ct);
            Status = $"Installing verified {app.Name} package…";
            app.Installed = await _installer.InstallZipAsync(app.Manifest, app.Release, temp, ct);
            installedSuccessfully = true;
            app.IsSelected = false;
            Status = $"{app.Name} {app.Installed.Version} is installed.";
            return null;
        }
        catch (OperationCanceledException) { throw; }
        catch (Exception ex) { _diagnostics.Error($"install.{app.Manifest.Id}", ex); return ex.Message; }
        finally
        {
            if (installedSuccessfully && temp is not null && File.Exists(temp))
            {
                try { File.Delete(temp); } catch (IOException) { } catch (UnauthorizedAccessException) { }
            }
        }
    }

    private Task RemoveOneAsync(AppItemViewModel app)
    {
        if (IsBusy) return Task.CompletedTask;
        try
        {
            var warning = _installer.Uninstall(app.Manifest.Id, app.Name);
            app.Installed = null;
            Status = warning ?? $"{app.Name} was removed.";
        }
        catch (Exception ex) { _diagnostics.Error($"remove.{app.Manifest.Id}", ex); Status = $"Could not remove {app.Name}: {ex.Message}"; }
        return Task.CompletedTask;
    }

    private void OpenOne(AppItemViewModel app)
    {
        try { if (app.Installed is not null) PortableInstallerService.Launch(app.Installed); }
        catch (Exception ex) { _diagnostics.Error($"launch.{app.Manifest.Id}", ex); Status = $"Could not open {app.Name}: {ex.Message}"; }
    }
}

public sealed class AppItemViewModel : ObservableObject
{
    private readonly Func<bool> _isBusy;
    private bool _isSelected;
    private InstalledApp? _installed;
    private ResolvedRelease? _release;
    private string? _error;
    public AppManifest Manifest { get; }
    public string Name => Manifest.Name;
    public string Description => Manifest.Description;
    public string Accent => Manifest.Accent;
    public Bitmap Icon { get; }
    public bool IsSelected { get => _isSelected; set => Set(ref _isSelected, value); }
    public InstalledApp? Installed { get => _installed; set { if (Set(ref _installed, value)) RaiseStatus(); } }
    public ResolvedRelease? Release { get => _release; set { if (Set(ref _release, value)) RaiseStatus(); } }
    public string? Error { get => _error; set { if (Set(ref _error, value)) Raise(nameof(ReleaseLabel)); } }
    public bool IsInstalled => Installed is not null && File.Exists(Installed.ExecutablePath);
    public bool HasInstalledVersion => IsInstalled;
    public bool HasRelease => Release is not null;
    public string InstalledBadge => IsInstalled ? $"✓ INSTALLED · {Installed!.Version}" : string.Empty;
    public string StateLabel
    {
        get
        {
            if (Installed is not null && !IsInstalled) return "REPAIR NEEDED";
            if (!IsInstalled) return "NOT INSTALLED";
            return Release is not null && VersionComparer.Compare(Release.Version, Installed!.Version) > 0 ? "UPDATE AVAILABLE" : "INSTALLED";
        }
    }
    public string VersionSummary
    {
        get
        {
            var installed = Installed?.Version ?? "—";
            var available = Release?.Version ?? "—";
            var size = Release?.Asset.Size > 0 ? FormatBytes(Release.Asset.Size) : "size unknown";
            return $"Installed {installed}  ·  Available {available}  ·  {size}";
        }
    }
    public string VerificationLabel => HasRelease ? "✓ SHA-256 REQUIRED" : string.Empty;
    public string CardBackground => Installed is not null && !IsInstalled ? "#2A1C1C" : IsInstalled ? "#19241F" : "#171923";
    public string CardBorder => Installed is not null && !IsInstalled ? "#D96A6A" : IsInstalled ? "#39C98A" : "#2B2E3C";
    public string ReleaseLabel => Release is null ? (Error is null ? "Checking…" : "Unavailable") : $"Available {Release.Version}";

    public string ActionLabel
    {
        get
        {
            if (Installed is not null && !IsInstalled) return "Repair";
            if (!IsInstalled) return "Install";
            if (Release is null) return "Reinstall";
            var comparison = VersionComparer.Compare(Release.Version, Installed!.Version);
            return comparison > 0 ? "Update" : comparison < 0 ? "Downgrade" : "Reinstall";
        }
    }

    public AsyncCommand InstallCommand { get; }
    public AsyncCommand RemoveCommand { get; }
    public RelayCommand OpenCommand { get; }

    public AppItemViewModel(AppManifest manifest, InstalledApp? installed, Func<bool> isBusy, Func<AppItemViewModel, Task> install, Func<AppItemViewModel, Task> remove, Action<AppItemViewModel> open)
    {
        Manifest = manifest; _installed = installed; _isBusy = isBusy;
        using (var icon = AssetLoader.Open(new Uri($"avares://ArtCraftSuite/Assets/icons/{manifest.Id}.png"))) Icon = new Bitmap(icon);
        InstallCommand = new(() => install(this), () => !_isBusy() && Release is not null);
        RemoveCommand = new(() => remove(this), () => !_isBusy() && IsInstalled);
        OpenCommand = new RelayCommand(() => open(this), () => !_isBusy() && IsInstalled);
    }

    public void NotifyCommands()
    {
        InstallCommand.Notify(); RemoveCommand.Notify(); OpenCommand.Notify();
    }

    private void RaiseStatus()
    {
        Raise(nameof(IsInstalled)); Raise(nameof(HasInstalledVersion)); Raise(nameof(InstalledBadge)); Raise(nameof(CardBackground));
        Raise(nameof(CardBorder)); Raise(nameof(ReleaseLabel)); Raise(nameof(ActionLabel)); Raise(nameof(HasRelease));
        Raise(nameof(StateLabel)); Raise(nameof(VersionSummary)); Raise(nameof(VerificationLabel));
        NotifyCommands();
    }

    private static string FormatBytes(long bytes)
    {
        string[] units = ["B", "KB", "MB", "GB"];
        var value = (double)bytes;
        var unit = 0;
        while (value >= 1024 && unit < units.Length - 1) { value /= 1024; unit++; }
        return $"{value:0.#} {units[unit]}";
    }
}

/// <summary>Compares release versions such as "1.2.0", "1.2.0-rc.4" or "unknown".</summary>
public static class VersionComparer
{
    public static int Compare(string? left, string? right)
    {
        var a = Parse(left);
        var b = Parse(right);
        if (a.Core is null || b.Core is null)
            // An unknown installed version is treated as older so the card offers an update.
            return a.Core is null && b.Core is null ? 0 : a.Core is null ? -1 : 1;
        var core = a.Core.CompareTo(b.Core);
        if (core != 0) return core;
        if (a.Pre is null || b.Pre is null) return a.Pre is null ? (b.Pre is null ? 0 : 1) : -1;
        return ComparePrerelease(a.Pre, b.Pre);
    }

    private static (Version? Core, string? Pre) Parse(string? value)
    {
        if (string.IsNullOrWhiteSpace(value)) return (null, null);
        var text = value.Trim().TrimStart('v', 'V');
        var plus = text.IndexOf('+');
        if (plus >= 0) text = text[..plus];
        var dash = text.IndexOf('-');
        var core = dash >= 0 ? text[..dash] : text;
        var pre = dash >= 0 ? text[(dash + 1)..] : null;
        if (!core.Contains('.')) core += ".0";
        return Version.TryParse(core, out var parsed) ? (Normalize(parsed), pre) : (null, null);
    }

    private static Version Normalize(Version v) => new(v.Major, v.Minor, Math.Max(v.Build, 0), Math.Max(v.Revision, 0));

    private static int ComparePrerelease(string a, string b)
    {
        var left = a.Split('.');
        var right = b.Split('.');
        for (var i = 0; i < Math.Min(left.Length, right.Length); i++)
        {
            var leftIsNumber = int.TryParse(left[i], out var l);
            var rightIsNumber = int.TryParse(right[i], out var r);
            var result = leftIsNumber && rightIsNumber ? l.CompareTo(r)
                : leftIsNumber ? -1 : rightIsNumber ? 1 : string.CompareOrdinal(left[i], right[i]);
            if (result != 0) return result;
        }
        return left.Length.CompareTo(right.Length);
    }
}

public sealed class RelayCommand(Action execute, Func<bool>? canExecute = null) : System.Windows.Input.ICommand
{
    public event EventHandler? CanExecuteChanged;
    public bool CanExecute(object? parameter) => canExecute?.Invoke() ?? true;
    public void Execute(object? parameter) => execute();
    public void Notify() => CanExecuteChanged?.Invoke(this, EventArgs.Empty);
}
