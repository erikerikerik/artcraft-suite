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
        DataContext = new MainViewModel(manifest, new GitHubReleaseService(), new PortableInstallerService());
    }
}
