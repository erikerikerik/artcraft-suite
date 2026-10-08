using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;

namespace ArtCraftSuite.Services;

public sealed class DiagnosticService
{
    private readonly object _gate = new();
    public string LogPath { get; }

    public DiagnosticService(string? root = null)
    {
        root ??= Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "ArtCraftSuite", "logs");
        Directory.CreateDirectory(root);
        LogPath = Path.Combine(root, "artcraft-suite.jsonl");
        Info("startup", "ArtCraft Suite started.");
    }

    public void Info(string eventName, string message) => Write("info", eventName, message, null);
    public void Error(string eventName, Exception error) => Write("error", eventName, error.Message, error.ToString());

    public string BuildReport(string channel, string status, IEnumerable<string> apps)
    {
        var version = typeof(DiagnosticService).Assembly.GetName().Version?.ToString(3) ?? "unknown";
        var report = new StringBuilder()
            .AppendLine("ArtCraft Suite diagnostic report")
            .AppendLine($"Generated: {DateTimeOffset.Now:O}")
            .AppendLine($"Version: {version}")
            .AppendLine($"OS: {RuntimeInformation.OSDescription} ({RuntimeInformation.OSArchitecture})")
            .AppendLine($"Runtime: {RuntimeInformation.FrameworkDescription}")
            .AppendLine($"Channel: {channel}")
            .AppendLine($"Status: {status}")
            .AppendLine("Applications:");
        foreach (var app in apps) report.AppendLine($"- {app}");
        report.AppendLine($"Log: {LogPath}");
        return report.ToString();
    }

    private void Write(string level, string eventName, string message, string? detail)
    {
        try
        {
            var line = JsonSerializer.Serialize(new { timestamp = DateTimeOffset.UtcNow, level, eventName, message, detail });
            lock (_gate) File.AppendAllText(LogPath, line + Environment.NewLine);
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException) { }
    }
}
