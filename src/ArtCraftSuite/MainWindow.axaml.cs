using Avalonia.Controls;
using ArtCraftSuite.Services;
using ArtCraftSuite.ViewModels;

namespace ArtCraftSuite;

public sealed partial class MainWindow : Window
{
    public MainWindow()
    {
        InitializeComponent();
        var manifest = ManifestService.Load();
        var diagnostics = new DiagnosticService();
        DataContext = new MainViewModel(manifest, new GitHubReleaseService(), new PortableInstallerService(), diagnostics, async text =>
        {
            var clipboard = TopLevel.GetTopLevel(this)?.Clipboard;
            if (clipboard is null) throw new InvalidOperationException("The system clipboard is unavailable.");
            await clipboard.SetTextAsync(text);
        });
    }
}
