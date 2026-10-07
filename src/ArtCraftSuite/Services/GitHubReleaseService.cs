using System.Net.Http.Json;
using System.Security.Cryptography;
using System.Text.Json;
using System.Text.Json.Serialization;
using System.Text.RegularExpressions;
using ArtCraftSuite.Models;

namespace ArtCraftSuite.Services;

public sealed class GitHubReleaseService
{
    private readonly HttpClient _client = new() { Timeout = TimeSpan.FromMinutes(10) };
    private static readonly JsonSerializerOptions JsonOptions = new() { PropertyNameCaseInsensitive = true };

    public GitHubReleaseService() => _client.DefaultRequestHeaders.UserAgent.ParseAdd("ArtCraft-Suite/0.1 (+https://github.com/)");

    public async Task<ResolvedRelease> ResolveAsync(AppManifest app, string channel, string platform, CancellationToken ct)
    {
        var releases = await _client.GetFromJsonAsync<List<ApiRelease>>(
            $"https://api.github.com/repos/{app.Repository}/releases?per_page=20", JsonOptions, ct) ?? [];
        var candidates = releases.Where(x => !x.Draft);
        if (channel.Equals("Stable", StringComparison.OrdinalIgnoreCase)) candidates = candidates.Where(x => !x.Prerelease);
        var release = candidates.FirstOrDefault() ?? throw new InvalidOperationException($"{app.Name} has no {channel.ToLowerInvariant()} release.");

        if (!app.AssetPatterns.TryGetValue(platform, out var template))
            throw new PlatformNotSupportedException($"{app.Name} has no package rule for {platform}.");
        var version = release.TagName.TrimStart('v', 'V');
        var pattern = template.Replace("{id}", Regex.Escape(app.Id), StringComparison.Ordinal)
            .Replace("{version}", Regex.Escape(version), StringComparison.Ordinal);
        var asset = release.Assets.FirstOrDefault(x => Regex.IsMatch(x.Name, pattern, RegexOptions.IgnoreCase | RegexOptions.CultureInvariant))
            ?? throw new InvalidOperationException($"No matching {platform} asset was found for {app.Name} {release.TagName}.");

        var sha = ParseDigest(asset.Digest) ?? await ReadChecksumAssetAsync(release, asset.Name, ct);
        if (sha is null) throw new InvalidOperationException($"{app.Name} {release.TagName} does not publish a usable SHA-256 for {asset.Name}.");
        return new(version, release.TagName, new(asset.Name, asset.BrowserDownloadUrl, asset.Size, asset.Digest), sha);
    }

    public async Task DownloadVerifiedAsync(ResolvedRelease release, string destination, IProgress<double>? progress, CancellationToken ct)
    {
        using var response = await _client.GetAsync(release.Asset.BrowserDownloadUrl, HttpCompletionOption.ResponseHeadersRead, ct);
        response.EnsureSuccessStatusCode();
        var total = response.Content.Headers.ContentLength ?? release.Asset.Size;
        await using var input = await response.Content.ReadAsStreamAsync(ct);
        await using var output = new FileStream(destination, FileMode.Create, FileAccess.Write, FileShare.None, 128 * 1024, true);
        using var hash = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
        var buffer = new byte[128 * 1024];
        long readTotal = 0;
        while (true)
        {
            var count = await input.ReadAsync(buffer, ct);
            if (count == 0) break;
            await output.WriteAsync(buffer.AsMemory(0, count), ct);
            hash.AppendData(buffer, 0, count);
            readTotal += count;
            if (total > 0) progress?.Report(readTotal * 100d / total);
        }
        var actual = Convert.ToHexString(hash.GetHashAndReset()).ToLowerInvariant();
        if (!CryptographicOperations.FixedTimeEquals(Convert.FromHexString(actual), Convert.FromHexString(release.Sha256)))
        {
            output.Close();
            File.Delete(destination);
            throw new InvalidDataException($"SHA-256 verification failed for {release.Asset.Name}.");
        }
    }

    private async Task<string?> ReadChecksumAssetAsync(ApiRelease release, string fileName, CancellationToken ct)
    {
        var checksums = release.Assets.FirstOrDefault(x => x.Name.Equals("SHA256SUMS.txt", StringComparison.OrdinalIgnoreCase));
        if (checksums is null) return null;
        var text = await _client.GetStringAsync(checksums.BrowserDownloadUrl, ct);
        foreach (var line in text.Split('\n', StringSplitOptions.TrimEntries | StringSplitOptions.RemoveEmptyEntries))
        {
            var match = Regex.Match(line, "^([a-fA-F0-9]{64})\\s+\\*?(.+)$");
            if (match.Success && match.Groups[2].Value.Equals(fileName, StringComparison.OrdinalIgnoreCase))
                return match.Groups[1].Value.ToLowerInvariant();
        }
        return null;
    }

    private static string? ParseDigest(string? digest)
    {
        if (digest is null || !digest.StartsWith("sha256:", StringComparison.OrdinalIgnoreCase)) return null;
        var value = digest[7..];
        return Regex.IsMatch(value, "^[a-fA-F0-9]{64}$") ? value.ToLowerInvariant() : null;
    }

    private sealed record ApiRelease(
        [property: JsonPropertyName("tag_name")] string TagName,
        [property: JsonPropertyName("draft")] bool Draft,
        [property: JsonPropertyName("prerelease")] bool Prerelease,
        [property: JsonPropertyName("assets")] List<ApiAsset> Assets);

    private sealed record ApiAsset(
        [property: JsonPropertyName("name")] string Name,
        [property: JsonPropertyName("browser_download_url")] string BrowserDownloadUrl,
        [property: JsonPropertyName("size")] long Size,
        [property: JsonPropertyName("digest")] string? Digest);
}
