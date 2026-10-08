using Avalonia.Controls;
using Avalonia.Controls.Primitives;
using ArtCraftSuite.Services;
using ArtCraftSuite.ViewModels;

namespace ArtCraftSuite;

public sealed partial class MainWindow : Window
{
    /// <summary>Narrowest a card may get before the grid drops a column; keeps the
    /// verification chip and all three buttons on one row.</summary>
    private const double MinCardWidth = 400;

    public MainWindow()
    {
        InitializeComponent();
        AppList.SizeChanged += (_, e) => UpdateColumns(e.NewSize.Width);
        AppList.Loaded += (_, _) => UpdateColumns(AppList.Bounds.Width);
        var manifest = ManifestService.Load();
        var diagnostics = new DiagnosticService();
        DataContext = new MainViewModel(manifest, new GitHubReleaseService(), new PortableInstallerService(), diagnostics, async text =>
        {
            var clipboard = TopLevel.GetTopLevel(this)?.Clipboard;
            if (clipboard is null) throw new InvalidOperationException("The system clipboard is unavailable.");
            await clipboard.SetTextAsync(text);
        });
    }

    private void UpdateColumns(double width)
    {
        if (width <= 0 || AppList.ItemsPanelRoot is not UniformGrid grid) return;
        var columns = CalculateColumnCount(width);
        if (grid.Columns != columns) grid.Columns = columns;
    }

    internal static int CalculateColumnCount(double width) => Math.Clamp((int)(width / MinCardWidth), 2, 4);
}
