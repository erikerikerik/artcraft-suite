using System.Net;
using System.Net.Http.Json;
using System.Net.Http.Headers;
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
    private readonly string _cacheRoot;
    private readonly Dictionary<string, List<ApiRelease>> _cache = new(StringComparer.OrdinalIgnoreCase);
    private readonly Dictionary<string, string> _etags = new(StringComparer.OrdinalIgnoreCase);
    private readonly HashSet<string> _fetchedThisSession = new(StringComparer.OrdinalIgnoreCase);
    private readonly SemaphoreSlim _cacheLock = new(1, 1);
    private static readonly JsonSerializerOptions JsonOptions = new() { PropertyNameCaseInsensitive = true };

    public GitHubReleaseService(HttpMessageHandler? handler = null, string? cacheRoot = null, string? token = null)
    {
        _client = handler is null ? new HttpClient() : new HttpClient(handler);
        _cacheRoot = cacheRoot ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "ArtCraftSuite", "cache", "releases");
        _client.Timeout = TimeSpan.FromMinutes(10);
        var version = typeof(GitHubReleaseService).Assembly.GetName().Version;
        var label = version is null ? "0.0.0" : $"{version.Major}.{version.Minor}.{version.Build}";
        _client.DefaultRequestHeaders.UserAgent.ParseAdd($"ArtCraft-Suite/{label} (+{ProjectUrl})");
        _client.DefaultRequestHeaders.Accept.ParseAdd("application/vnd.github+json");
        _client.DefaultRequestHeaders.Add("X-GitHub-Api-Version", "2022-11-28");
        token ??= Environment.GetEnvironmentVariable("ARTCRAFT_GITHUB_TOKEN");
        if (!string.IsNullOrWhiteSpace(token)) _client.DefaultRequestHeaders.Authorization = new AuthenticationHeaderValue("Bearer", token.Trim());
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
        catch (OperationCanceledException) when (ct.IsCancellationRequested) { throw; }
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
        var offset = File.Exists(destination) ? new FileInfo(destination).Length : 0;
        using var request = new HttpRequestMessage(HttpMethod.Get, release.Asset.BrowserDownloadUrl);
        if (offset > 0) request.Headers.Range = new RangeHeaderValue(offset, null);
        using var response = await _client.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, ct);

        if (offset > 0 && response.StatusCode == HttpStatusCode.RequestedRangeNotSatisfiable)
        {
            if (await VerifyFileAsync(destination, release, ct)) { progress?.Report(100); return; }
            File.Delete(destination);
            await DownloadVerifiedAsync(release, destination, progress, ct);
            return;
        }

        response.EnsureSuccessStatusCode();
        var resumed = offset > 0 && response.StatusCode == HttpStatusCode.PartialContent
            && response.Content.Headers.ContentRange?.From == offset;
        if (!resumed) offset = 0;
        var expectedLength = response.Content.Headers.ContentRange?.Length
            ?? (release.Asset.Size > 0 ? release.Asset.Size : response.Content.Headers.ContentLength);
        await using (var input = await response.Content.ReadAsStreamAsync(ct))
        await using (var output = new FileStream(destination, resumed ? FileMode.Append : FileMode.Create, FileAccess.Write, FileShare.None, 128 * 1024, true))
        {
            var buffer = new byte[128 * 1024];
            long received = offset;
            while (true)
            {
                var count = await input.ReadAsync(buffer, ct);
                if (count == 0) break;
                await output.WriteAsync(buffer.AsMemory(0, count), ct);
                received += count;
                if (expectedLength > 0) progress?.Report(Math.Min(99, received * 100d / expectedLength.Value));
            }
            await output.FlushAsync(ct);
        }
        var finalLength = new FileInfo(destination).Length;
        if (expectedLength is > 0 && finalLength != expectedLength.Value)
            throw new InvalidDataException($"Download is incomplete: expected {expectedLength.Value:N0} bytes, received {finalLength:N0}. It will resume on retry.");
        if (!await VerifyFileAsync(destination, release, ct))
        {
            File.Delete(destination);
            throw new InvalidDataException($"SHA-256 verification failed for {release.Asset.Name}; the partial file was discarded.");
        }
        progress?.Report(100);
    }

    private static async Task<bool> VerifyFileAsync(string path, ResolvedRelease release, CancellationToken ct)
    {
        if (!File.Exists(path) || release.Asset.Size > 0 && new FileInfo(path).Length != release.Asset.Size) return false;
        await using var input = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read, 128 * 1024, true);
        var actual = await SHA256.HashDataAsync(input, ct);
        return CryptographicOperations.FixedTimeEquals(actual, Convert.FromHexString(release.Sha256));
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
            if (!forceRefresh && _fetchedThisSession.Contains(repository) && _cache.TryGetValue(repository, out var memoryCached)) return memoryCached;
            LoadDiskCache(repository);
            using var request = new HttpRequestMessage(HttpMethod.Get, $"https://api.github.com/repos/{repository}/releases?per_page=20");
            if (_etags.TryGetValue(repository, out var etag)) request.Headers.IfNoneMatch.Add(EntityTagHeaderValue.Parse(etag));
            HttpResponseMessage response;
            try { response = await _client.SendAsync(request, ct); }
            catch (HttpRequestException) when (_cache.TryGetValue(repository, out var offlineCached)) { return offlineCached; }
            catch (OperationCanceledException) when (!ct.IsCancellationRequested && _cache.TryGetValue(repository, out var timeoutCached)) { return timeoutCached; }
            using (response)
            {
                if (response.StatusCode == HttpStatusCode.NotModified && _cache.TryGetValue(repository, out var notModified))
                {
                    _fetchedThisSession.Add(repository);
                    return notModified;
                }
                if (IsRateLimited(response) && _cache.TryGetValue(repository, out var rateLimitedCached)) return rateLimitedCached;
                ThrowIfRateLimited(response);
                if (response.StatusCode == HttpStatusCode.NotFound)
                    throw new InvalidOperationException($"GitHub repository {repository} was not found.");
                response.EnsureSuccessStatusCode();
                var releases = await response.Content.ReadFromJsonAsync<List<ApiRelease>>(JsonOptions, ct) ?? [];
                _cache[repository] = releases;
                _fetchedThisSession.Add(repository);
                if (response.Headers.ETag is not null) _etags[repository] = response.Headers.ETag.ToString();
                SaveDiskCache(repository, releases);
                return releases;
            }
        }
        finally { _cacheLock.Release(); }
    }

    private string CachePath(string repository) => Path.Combine(_cacheRoot, repository.Replace('/', '-') + ".json");

    private void LoadDiskCache(string repository)
    {
        if (_cache.ContainsKey(repository)) return;
        var path = CachePath(repository);
        if (!File.Exists(path)) return;
        try
        {
            var envelope = JsonSerializer.Deserialize<CacheEnvelope>(File.ReadAllText(path), JsonOptions);
            if (envelope is null || envelope.Releases.Count == 0) return;
            _cache[repository] = envelope.Releases;
            if (!string.IsNullOrWhiteSpace(envelope.ETag)) _etags[repository] = envelope.ETag;
        }
        catch (Exception ex) when (ex is IOException or JsonException) { }
    }

    private void SaveDiskCache(string repository, List<ApiRelease> releases)
    {
        try
        {
            Directory.CreateDirectory(_cacheRoot);
            var path = CachePath(repository);
            var temp = path + $".{Guid.NewGuid():N}.tmp";
            try
            {
                File.WriteAllText(temp, JsonSerializer.Serialize(new CacheEnvelope(_etags.GetValueOrDefault(repository), releases), JsonOptions));
                File.Move(temp, path, true);
            }
            finally { try { File.Delete(temp); } catch { } }
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException) { }
    }

    private static void ThrowIfRateLimited(HttpResponseMessage response)
    {
        if (!IsRateLimited(response)) return;
        var message = "GitHub's hourly API limit for update checks has been reached.";
        if (long.TryParse(Header(response, "X-RateLimit-Reset"), out var reset))
            message += $" It resets at {DateTimeOffset.FromUnixTimeSeconds(reset).ToLocalTime():t}.";
        throw new InvalidOperationException(message);
    }

    private static bool IsRateLimited(HttpResponseMessage response) =>
        response.StatusCode == HttpStatusCode.TooManyRequests
        || response.StatusCode == HttpStatusCode.Forbidden && Header(response, "X-RateLimit-Remaining") == "0";

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

    private sealed record CacheEnvelope(string? ETag, List<ApiRelease> Releases);
}
