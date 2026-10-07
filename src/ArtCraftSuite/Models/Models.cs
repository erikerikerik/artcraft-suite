using System.Text.Json.Serialization;

namespace ArtCraftSuite.Models;

public sealed record SuiteManifest(
    [property: JsonPropertyName("schemaVersion")] int SchemaVersion,
    [property: JsonPropertyName("apps")] IReadOnlyList<AppManifest> Apps);

public sealed record AppManifest(
    [property: JsonPropertyName("id")] string Id,
    [property: JsonPropertyName("name")] string Name,
    [property: JsonPropertyName("description")] string Description,
    [property: JsonPropertyName("repository")] string Repository,
    [property: JsonPropertyName("accent")] string Accent,
    [property: JsonPropertyName("assetPatterns")] Dictionary<string, string> AssetPatterns);

public sealed record GitHubRelease(string TagName, bool Draft, bool Prerelease, IReadOnlyList<GitHubAsset> Assets);

public sealed record GitHubAsset(string Name, string BrowserDownloadUrl, long Size, string? Digest);

public sealed record ResolvedRelease(string Version, string Tag, GitHubAsset Asset, string Sha256);

public sealed record InstalledApp(string Id, string Version, string ExecutablePath, string SourceAsset, string Sha256, DateTimeOffset InstalledAt);

public sealed class InstallState
{
    public Dictionary<string, InstalledApp> Apps { get; init; } = new(StringComparer.OrdinalIgnoreCase);
}
