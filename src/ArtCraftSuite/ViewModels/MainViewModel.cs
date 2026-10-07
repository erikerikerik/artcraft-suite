using System.Collections.ObjectModel;
using ArtCraftSuite.Models;
using ArtCraftSuite.Services;

namespace ArtCraftSuite.ViewModels;

public sealed class MainViewModel : ObservableObject
{
    private readonly GitHubReleaseService _releases;
    private readonly PortableInstallerService _installer;
    private bool _isBusy;
    private double _progress;
    private string _status = "Ready. Choose apps, then install.";
    private string _channel = "Stable";

    public ObservableCollection<AppItemViewModel> Apps { get; } = [];
    public IReadOnlyList<string> Channels { get; } = ["Stable", "Latest"];
    public AsyncCommand RefreshCommand { get; }
    public AsyncCommand InstallSelectedCommand { get; }
    public bool IsBusy { get => _isBusy; private set => Set(ref _isBusy, value); }
    public double Progress { get => _progress; private set => Set(ref _progress, value); }
    public string Status { get => _status; private set => Set(ref _status, value); }
    public string Channel
    {
        get => _channel;
        set { if (Set(ref _channel, value)) _ = RefreshAsync(); }
    }

    public MainViewModel(SuiteManifest manifest, GitHubReleaseService releases, PortableInstallerService installer)
    {
        _releases = releases;
        _installer = installer;
        var state = installer.LoadState();
        foreach (var item in manifest.Apps)
        {
            state.Apps.TryGetValue(item.Id, out var installed);
            var vm = new AppItemViewModel(item, installed, InstallOneAsync, RemoveOneAsync, OpenOne);
            Apps.Add(vm);
        }
        RefreshCommand = new(RefreshAsync, () => !IsBusy);
        InstallSelectedCommand = new(InstallSelectedAsync, () => !IsBusy && Apps.Any(x => x.IsSelected));
        foreach (var app in Apps) app.PropertyChanged += (_, e) => { if (e.PropertyName == nameof(AppItemViewModel.IsSelected)) InstallSelectedCommand.Notify(); };
        _ = RefreshAsync();
    }

    private string Platform => OperatingSystem.IsWindows() ? "windows-x64" : OperatingSystem.IsMacOS() && System.Runtime.InteropServices.RuntimeInformation.OSArchitecture == System.Runtime.InteropServices.Architecture.Arm64 ? "macos-arm64" : "unsupported";

    private async Task RefreshAsync()
    {
        if (IsBusy) return;
        IsBusy = true; Progress = 0; Status = $"Checking {Channel.ToLowerInvariant()} releases…";
        try
        {
            var completed = 0;
            foreach (var app in Apps)
            {
                try { app.Release = await _releases.ResolveAsync(app.Manifest, Channel, Platform, CancellationToken.None); app.Error = null; }
                catch (Exception ex) { app.Release = null; app.Error = ex.Message; }
                Progress = ++completed * 100d / Apps.Count;
            }
            var available = Apps.Count(x => x.Release is not null);
            Status = available == Apps.Count ? $"All {available} apps are ready." : $"{available} of {Apps.Count} releases are available. Hover unavailable items for details.";
        }
        catch (Exception ex) { Status = $"Refresh failed: {ex.Message}"; }
        finally { IsBusy = false; Progress = 0; RefreshCommand.Notify(); InstallSelectedCommand.Notify(); }
    }

    private async Task InstallSelectedAsync()
    {
        foreach (var app in Apps.Where(x => x.IsSelected).ToList()) await InstallOneAsync(app);
    }

    private async Task InstallOneAsync(AppItemViewModel app)
    {
        if (IsBusy) return;
        IsBusy = true; Progress = 0;
        string? temp = null;
        try
        {
            app.Release ??= await _releases.ResolveAsync(app.Manifest, Channel, Platform, CancellationToken.None);
            temp = Path.Combine(Path.GetTempPath(), $"artcraft-suite-{Guid.NewGuid():N}.zip");
            Status = $"Downloading {app.Name} {app.Release.Version}…";
            await _releases.DownloadVerifiedAsync(app.Release, temp, new Progress<double>(x => Progress = x), CancellationToken.None);
            Status = $"Installing verified {app.Name} package…";
            app.Installed = await _installer.InstallZipAsync(app.Manifest, app.Release, temp, CancellationToken.None);
            app.IsSelected = false;
            Status = $"{app.Name} {app.Installed.Version} is installed.";
        }
        catch (Exception ex) { Status = $"Could not install {app.Name}: {ex.Message}"; }
        finally
        {
            if (temp is not null && File.Exists(temp)) File.Delete(temp);
            IsBusy = false; Progress = 0; RefreshCommand.Notify(); InstallSelectedCommand.Notify();
        }
    }

    private Task RemoveOneAsync(AppItemViewModel app)
    {
        try { _installer.Uninstall(app.Manifest.Id); app.Installed = null; Status = $"{app.Name} was removed."; }
        catch (Exception ex) { Status = $"Could not remove {app.Name}: {ex.Message}"; }
        return Task.CompletedTask;
    }

    private void OpenOne(AppItemViewModel app)
    {
        try { if (app.Installed is not null) PortableInstallerService.Launch(app.Installed); }
        catch (Exception ex) { Status = $"Could not open {app.Name}: {ex.Message}"; }
    }
}

public sealed class AppItemViewModel : ObservableObject
{
    private bool _isSelected;
    private InstalledApp? _installed;
    private ResolvedRelease? _release;
    private string? _error;
    public AppManifest Manifest { get; }
    public string Name => Manifest.Name;
    public string Description => Manifest.Description;
    public string Accent => Manifest.Accent;
    public string Initials => string.Concat(Name.Where(char.IsUpper).Take(2));
    public bool IsSelected { get => _isSelected; set => Set(ref _isSelected, value); }
    public InstalledApp? Installed { get => _installed; set { if (Set(ref _installed, value)) RaiseStatus(); } }
    public ResolvedRelease? Release { get => _release; set { if (Set(ref _release, value)) RaiseStatus(); } }
    public string? Error { get => _error; set { if (Set(ref _error, value)) Raise(nameof(ReleaseLabel)); } }
    public bool IsInstalled => Installed is not null && File.Exists(Installed.ExecutablePath);
    public bool HasInstalledVersion => IsInstalled;
    public string InstalledBadge => IsInstalled ? $"Installed {Installed!.Version}" : string.Empty;
    public string ReleaseLabel => Release is null ? (Error is null ? "Checking…" : "Unavailable") : $"Available {Release.Version}";
    public string ActionLabel => IsInstalled && Release?.Version != Installed?.Version ? "Update" : IsInstalled ? "Reinstall" : "Install";
    public AsyncCommand InstallCommand { get; }
    public AsyncCommand RemoveCommand { get; }
    public System.Windows.Input.ICommand OpenCommand { get; }

    public AppItemViewModel(AppManifest manifest, InstalledApp? installed, Func<AppItemViewModel, Task> install, Func<AppItemViewModel, Task> remove, Action<AppItemViewModel> open)
    {
        Manifest = manifest; _installed = installed;
        InstallCommand = new(() => install(this), () => Release is not null);
        RemoveCommand = new(() => remove(this), () => IsInstalled);
        OpenCommand = new RelayCommand(() => open(this), () => IsInstalled);
    }
    private void RaiseStatus()
    {
        Raise(nameof(IsInstalled)); Raise(nameof(HasInstalledVersion)); Raise(nameof(InstalledBadge)); Raise(nameof(ReleaseLabel)); Raise(nameof(ActionLabel));
        InstallCommand.Notify(); RemoveCommand.Notify();
    }
}

public sealed class RelayCommand(Action execute, Func<bool>? canExecute = null) : System.Windows.Input.ICommand
{
    public event EventHandler? CanExecuteChanged;
    public bool CanExecute(object? parameter) => canExecute?.Invoke() ?? true;
    public void Execute(object? parameter) => execute();
    public void Notify() => CanExecuteChanged?.Invoke(this, EventArgs.Empty);
}
