using System.Net;
using System.Net.Http.Json;
using System.Security.Cryptography;
using System.Text.Json;
using System.Text.Json.Serialization;
using System.Text.RegularExpressions;
using ArtCraftSuite.Models;

namespace ArtCraftSuite.Services;

public sealed class GitHubReleaseService
{
    private const string ProjectUrl = "https://github.com/erikerikerik/artcraft-suite";
    private readonly HttpClient _client;
    private readonly Dictionary<string, List<ApiRelease>> _cache = new(StringComparer.OrdinalIgnoreCase);
    private readonly SemaphoreSlim _cacheLock = new(1, 1);
    private static readonly JsonSerializerOptions JsonOptions = new() { PropertyNameCaseInsensitive = true };

    public GitHubReleaseService(HttpMessageHandler? handler = null)
    {
        _client = handler is null ? new HttpClient() : new HttpClient(handler);
        _client.Timeout = TimeSpan.FromMinutes(10);
        var version = typeof(GitHubReleaseService).Assembly.GetName().Version;
        var label = version is null ? "0.0.0" : $"{version.Major}.{version.Minor}.{version.Build}";
        _client.DefaultRequestHeaders.UserAgent.ParseAdd($"ArtCraft-Suite/{label} (+{ProjectUrl})");
        _client.DefaultRequestHeaders.Accept.ParseAdd("application/vnd.github+json");
        _client.DefaultRequestHeaders.Add("X-GitHub-Api-Version", "2022-11-28");
    }

    public async Task<ResolvedRelease> ResolveAsync(AppManifest app, string channel, string platform, bool forceRefresh, CancellationToken ct)
    {
        if (!app.AssetPatterns.TryGetValue(platform, out var template))
            throw new PlatformNotSupportedException($"{app.Name} has no package rule for {platform}.");

        try
        {
            var releases = await GetReleasesAsync(app.Repository, forceRefresh, ct);
            return await ResolveFromApiAsync(app, channel, template, platform, releases, ct);
        }
        catch (OperationCanceledException) { throw; }
        catch (Exception apiError) when (channel.Equals("Stable", StringComparison.OrdinalIgnoreCase))
        {
            try { return await ResolveStableFallbackAsync(app, template, ct); }
            catch (Exception fallbackError)
            {
                throw new InvalidOperationException(
                    $"GitHub API check failed ({apiError.Message}); stable-channel fallback also failed ({fallbackError.Message}).",
                    new AggregateException(apiError, fallbackError));
            }
        }
    }

    public async Task DownloadVerifiedAsync(ResolvedRelease release, string destination, IProgress<double>? progress, CancellationToken ct)
    {
        try
        {
            using var response = await _client.GetAsync(release.Asset.BrowserDownloadUrl, HttpCompletionOption.ResponseHeadersRead, ct);
            response.EnsureSuccessStatusCode();
            var expectedLength = response.Content.Headers.ContentLength ?? (release.Asset.Size > 0 ? release.Asset.Size : null);
            string actual;
            long readTotal = 0;
            await using (var input = await response.Content.ReadAsStreamAsync(ct))
            await using (var output = new FileStream(destination, FileMode.Create, FileAccess.Write, FileShare.None, 128 * 1024, true))
            {
                using var hash = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
                var buffer = new byte[128 * 1024];
                while (true)
                {
                    var count = await input.ReadAsync(buffer, ct);
                    if (count == 0) break;
                    await output.WriteAsync(buffer.AsMemory(0, count), ct);
                    hash.AppendData(buffer, 0, count);
                    readTotal += count;
                    if (expectedLength > 0) progress?.Report(Math.Min(100, readTotal * 100d / expectedLength.Value));
                }
                await output.FlushAsync(ct);
                actual = Convert.ToHexString(hash.GetHashAndReset()).ToLowerInvariant();
            }
            if (expectedLength is > 0 && readTotal != expectedLength.Value)
                throw new InvalidDataException($"Download was incomplete: expected {expectedLength.Value:N0} bytes, received {readTotal:N0}.");
            if (!CryptographicOperations.FixedTimeEquals(Convert.FromHexString(actual), Convert.FromHexString(release.Sha256)))
                throw new InvalidDataException($"SHA-256 verification failed for {release.Asset.Name}.");
            progress?.Report(100);
        }
        catch
        {
            try { if (File.Exists(destination)) File.Delete(destination); } catch { }
            throw;
        }
    }

    private async Task<ResolvedRelease> ResolveFromApiAsync(AppManifest app, string channel, string template, string platform, List<ApiRelease> releases, CancellationToken ct)
    {
        var candidates = releases.Where(x => !x.Draft);
        if (channel.Equals("Stable", StringComparison.OrdinalIgnoreCase)) candidates = candidates.Where(x => !x.Prerelease);
        var foundRelease = false;
        foreach (var release in candidates)
        {
            foundRelease = true;
            var version = release.TagName.TrimStart('v', 'V');
            var pattern = BuildAssetPattern(template, app.EffectivePackageId, version);
            var asset = release.Assets.FirstOrDefault(x => Regex.IsMatch(x.Name, pattern, RegexOptions.IgnoreCase | RegexOptions.CultureInvariant));
            if (asset is null) continue;
            var sha = ParseDigest(asset.Digest) ?? await ReadChecksumAssetAsync(release.Assets, asset.Name, ct);
            if (sha is null) throw new InvalidOperationException($"{app.Name} {release.TagName} does not publish a usable SHA-256 for {asset.Name}.");
            return new(version, release.TagName, new(asset.Name, asset.BrowserDownloadUrl, asset.Size, asset.Digest), sha);
        }
        if (!foundRelease) throw new InvalidOperationException($"{app.Name} has no {channel.ToLowerInvariant()} release.");
        throw new InvalidOperationException($"No {channel.ToLowerInvariant()} {app.Name} release includes a {platform} package.");
    }

    private async Task<ResolvedRelease> ResolveStableFallbackAsync(AppManifest app, string template, CancellationToken ct)
    {
        using var response = await _client.GetAsync($"https://github.com/{app.Repository}/releases/latest", HttpCompletionOption.ResponseHeadersRead, ct);
        response.EnsureSuccessStatusCode();
        var finalUrl = response.RequestMessage?.RequestUri?.AbsoluteUri
            ?? throw new InvalidOperationException("GitHub did not return the latest release URL.");
        const string marker = "/releases/tag/";
        var markerIndex = finalUrl.IndexOf(marker, StringComparison.OrdinalIgnoreCase);
        if (markerIndex < 0) throw new InvalidOperationException("GitHub did not redirect to a stable release tag.");
        var tag = Uri.UnescapeDataString(finalUrl[(markerIndex + marker.Length)..].Trim('/'));
        var version = tag.TrimStart('v', 'V');
        var assetName = BuildConcreteAssetName(template, app.EffectivePackageId, version);
        var root = $"https://github.com/{app.Repository}/releases/download/{Uri.EscapeDataString(tag)}";
        var checksums = await _client.GetStringAsync($"{root}/SHA256SUMS.txt", ct);
        var sha = ChecksumForFile(checksums, assetName)
            ?? throw new InvalidOperationException($"{app.Name} does not publish a SHA-256 for {assetName}.");
        return new(version, tag, new(assetName, $"{root}/{Uri.EscapeDataString(assetName)}", 0, $"sha256:{sha}"), sha);
    }

    private async Task<List<ApiRelease>> GetReleasesAsync(string repository, bool forceRefresh, CancellationToken ct)
    {
        await _cacheLock.WaitAsync(ct);
        try
        {
            if (!forceRefresh && _cache.TryGetValue(repository, out var cached)) return cached;
            using var response = await _client.GetAsync($"https://api.github.com/repos/{repository}/releases?per_page=20", ct);
            ThrowIfRateLimited(response);
            if (response.StatusCode == HttpStatusCode.NotFound)
                throw new InvalidOperationException($"GitHub repository {repository} was not found.");
            response.EnsureSuccessStatusCode();
            var releases = await response.Content.ReadFromJsonAsync<List<ApiRelease>>(JsonOptions, ct) ?? [];
            _cache[repository] = releases;
            return releases;
        }
        finally { _cacheLock.Release(); }
    }

    private static void ThrowIfRateLimited(HttpResponseMessage response)
    {
        if (response.StatusCode is not (HttpStatusCode.Forbidden or HttpStatusCode.TooManyRequests)) return;
        var remaining = Header(response, "X-RateLimit-Remaining");
        if (response.StatusCode == HttpStatusCode.Forbidden && remaining != "0") return;
        var message = "GitHub's hourly API limit for update checks has been reached.";
        if (long.TryParse(Header(response, "X-RateLimit-Reset"), out var reset))
            message += $" It resets at {DateTimeOffset.FromUnixTimeSeconds(reset).ToLocalTime():t}.";
        throw new InvalidOperationException(message);
    }

    private static string? Header(HttpResponseMessage response, string name) =>
        response.Headers.TryGetValues(name, out var values) ? values.FirstOrDefault() : null;

    private async Task<string?> ReadChecksumAssetAsync(IReadOnlyList<ApiAsset> assets, string fileName, CancellationToken ct)
    {
        var checksums = assets.FirstOrDefault(x => x.Name.Equals("SHA256SUMS.txt", StringComparison.OrdinalIgnoreCase));
        return checksums is null ? null : ChecksumForFile(await _client.GetStringAsync(checksums.BrowserDownloadUrl, ct), fileName);
    }

    internal static string BuildAssetPattern(string template, string packageId, string version) =>
        template.Replace("{id}", Regex.Escape(packageId), StringComparison.Ordinal)
            .Replace("{version}", Regex.Escape(version), StringComparison.Ordinal);

    internal static string BuildConcreteAssetName(string template, string packageId, string version)
    {
        var name = template.TrimStart('^').TrimEnd('$')
            .Replace("{id}", packageId, StringComparison.Ordinal)
            .Replace("{version}", version, StringComparison.Ordinal)
            .Replace("\\.", ".", StringComparison.Ordinal);
        if (Regex.IsMatch(name, @"[\[\](){}+*?|]"))
            throw new InvalidOperationException("Package rule is too complex for the stable-channel fallback.");
        return name;
    }

    internal static string? ChecksumForFile(string text, string fileName)
    {
        foreach (var line in text.Split('\n', StringSplitOptions.TrimEntries | StringSplitOptions.RemoveEmptyEntries))
        {
            var match = Regex.Match(line, "^([a-fA-F0-9]{64})\\s+\\*?(.+)$");
            if (match.Success && match.Groups[2].Value.Equals(fileName, StringComparison.OrdinalIgnoreCase))
                return match.Groups[1].Value.ToLowerInvariant();
        }
        return null;
    }

    internal static string? ParseDigest(string? digest)
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
