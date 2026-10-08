using Avalonia;
using Avalonia.Controls;
using Avalonia.Controls.ApplicationLifetimes;
using Avalonia.Layout;
using Avalonia.Markup.Xaml;

namespace ArtCraftSuite;

public sealed partial class App : Application
{
    public override void Initialize() => AvaloniaXamlLoader.Load(this);

    public override void OnFrameworkInitializationCompleted()
    {
        if (ApplicationLifetime is IClassicDesktopStyleApplicationLifetime desktop)
        {
            try
            {
                desktop.MainWindow = new MainWindow();
            }
            catch (IOException error)
            {
                desktop.MainWindow = BuildStartupErrorWindow(error);
            }
        }
        base.OnFrameworkInitializationCompleted();
    }

    private static Window BuildStartupErrorWindow(IOException error)
    {
        var closeButton = new Button
        {
            Content = "Close",
            HorizontalAlignment = HorizontalAlignment.Right,
            MinWidth = 90
        };
        var window = new Window
        {
            Title = "ArtCraft Suite could not start",
            Width = 520,
            Height = 220,
            MinWidth = 420,
            MinHeight = 180,
            WindowStartupLocation = WindowStartupLocation.CenterScreen,
            Content = new StackPanel
            {
                Margin = new Thickness(24),
                Spacing = 16,
                Children =
                {
                    new TextBlock
                    {
                        Text = "ArtCraft Suite could not read its installation settings. " +
                            "Close other programs using the settings file, then try again.",
                        TextWrapping = Avalonia.Media.TextWrapping.Wrap
                    },
                    new TextBlock
                    {
                        Text = error.Message,
                        TextWrapping = Avalonia.Media.TextWrapping.Wrap
                    },
                    closeButton
                }
            }
        };
        closeButton.Click += (_, _) => window.Close();
        return window;
    }
}
