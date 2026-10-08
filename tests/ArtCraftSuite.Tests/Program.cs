using System.IO.Compression;
using System.Net;
using System.Text.Json;
using ArtCraftSuite.Models;
using ArtCraftSuite.Services;
using ArtCraftSuite.ViewModels;

var tests = new (string Name, Func<Task> Run)[]
{
    ("manifest maps PrintCraft to PDFCraft packages", TestManifestAlias),
    ("release helpers select exact package and checksum", TestReleaseHelpers),
    ("stable channel falls back when GitHub API is rate limited", TestStableFallback),
    ("version comparison handles prereleases", TestVersions),
    ("ZIP extraction rejects traversal", TestTraversal),
    ("ZIP extraction rejects symbolic links", TestSymbolicLink),
    ("PDFCraft executable installs transactionally as PrintCraft", TestPdfCraftInstall)
};

var failures = 0;
foreach (var test in tests)
{
    try { await test.Run(); Console.WriteLine($"PASS {test.Name}"); }
    catch (Exception ex) { failures++; Console.Error.WriteLine($"FAIL {test.Name}: {ex.Message}"); }
}
Console.WriteLine($"{tests.Length - failures}/{tests.Length} tests passed.");
return failures == 0 ? 0 : 1;

static Task TestManifestAlias()
{
    var path = Path.Combine(AppContext.BaseDirectory, "manifest", "apps.json");
    var manifest = JsonSerializer.Deserialize<SuiteManifest>(File.ReadAllText(path), new JsonSerializerOptions { PropertyNameCaseInsensitive = true })!;
    var print = manifest.Apps.Single(x => x.Id == "printcraft");
    Equal("storytold/printcraft", print.Repository);
    Equal("pdfcraft", print.EffectivePackageId);
    return Task.CompletedTask;
}

static Task TestReleaseHelpers()
{
    const string template = "^{id}-{version}-windows-x64-portable\\.zip$";
    Equal("pdfcraft-0.5.0-windows-x64-portable.zip", GitHubReleaseService.BuildConcreteAssetName(template, "pdfcraft", "0.5.0"));
    var pattern = GitHubReleaseService.BuildAssetPattern(template, "pdfcraft", "0.5.0");
    True(System.Text.RegularExpressions.Regex.IsMatch("pdfcraft-0.5.0-windows-x64-portable.zip", pattern));
    var hash = new string('a', 64);
    Equal(hash, GitHubReleaseService.ChecksumForFile($"{new string('b', 64)}  other.zip\n{hash} *pdfcraft.zip\n", "pdfcraft.zip"));
    Equal(hash, GitHubReleaseService.ParseDigest($"sha256:{hash}"));
    return Task.CompletedTask;
}

static async Task TestStableFallback()
{
    var hash = new string('c', 64);
    var handler = new StubHandler(request =>
    {
        if (request.RequestUri!.Host == "api.github.com")
        {
            var limited = new HttpResponseMessage(HttpStatusCode.Forbidden);
            limited.Headers.Add("X-RateLimit-Remaining", "0");
            return limited;
        }
        if (request.RequestUri.AbsolutePath.EndsWith("/releases/latest", StringComparison.Ordinal))
        {
            return new HttpResponseMessage(HttpStatusCode.OK)
            {
                RequestMessage = new HttpRequestMessage(HttpMethod.Get, "https://github.com/storytold/printcraft/releases/tag/v0.5.0"),
                Content = new StringContent(string.Empty)
            };
        }
        if (request.RequestUri.AbsolutePath.EndsWith("/SHA256SUMS.txt", StringComparison.Ordinal))
            return new HttpResponseMessage(HttpStatusCode.OK) { Content = new StringContent($"{hash}  pdfcraft-0.5.0-windows-x64-portable.zip\n") };
        throw new InvalidOperationException($"Unexpected request: {request.RequestUri}");
    });
    var service = new GitHubReleaseService(handler);
    var app = new AppManifest("printcraft", "PrintCraft", "PDF", "storytold/printcraft", "pdfcraft", "#fff",
        new Dictionary<string, string> { ["windows-x64"] = "^{id}-{version}-windows-x64-portable\\.zip$" });
    var release = await service.ResolveAsync(app, "Stable", "windows-x64", true, default);
    Equal("0.5.0", release.Version);
    Equal("pdfcraft-0.5.0-windows-x64-portable.zip", release.Asset.Name);
    Equal(hash, release.Sha256);
}

static Task TestVersions()
{
    True(VersionComparer.Compare("1.2.0", "1.2.0-rc.1") > 0);
    True(VersionComparer.Compare("1.2.0-rc.2", "1.2.0-rc.10") < 0);
    True(VersionComparer.Compare("1.0", "unknown") > 0);
    return Task.CompletedTask;
}

static Task TestTraversal()
{
    using var area = new TempArea();
    var archive = Path.Combine(area.Path, "unsafe.zip");
    using (var zip = ZipFile.Open(archive, ZipArchiveMode.Create)) zip.CreateEntry("../escape.txt");
    Throws<InvalidDataException>(() => PortableInstallerService.ExtractSafely(archive, Path.Combine(area.Path, "out"), default));
    return Task.CompletedTask;
}

static Task TestSymbolicLink()
{
    using var area = new TempArea();
    var archive = Path.Combine(area.Path, "symlink.zip");
    using (var zip = ZipFile.Open(archive, ZipArchiveMode.Create))
    {
        var link = zip.CreateEntry("link");
        link.ExternalAttributes = 0xA000 << 16;
    }
    Throws<InvalidDataException>(() => PortableInstallerService.ExtractSafely(archive, Path.Combine(area.Path, "out"), default));
    return Task.CompletedTask;
}

static async Task TestPdfCraftInstall()
{
    using var area = new TempArea();
    var archive = Path.Combine(area.Path, "pdfcraft.zip");
    using (var zip = ZipFile.Open(archive, ZipArchiveMode.Create))
    {
        var entry = zip.CreateEntry("PDFCraft/pdfcraft.exe");
        await using var output = entry.Open();
        await output.WriteAsync("test executable"u8.ToArray());
    }
    var app = new AppManifest("printcraft", "PrintCraft", "PDF", "storytold/printcraft", "pdfcraft", "#fff",
        new Dictionary<string, string> { ["windows-x64"] = "^{id}-{version}-windows-x64-portable\\.zip$" });
    var release = new ResolvedRelease("0.3.0", "v0.3.0", new GitHubAsset(Path.GetFileName(archive), "https://invalid/", 0, null), new string('0', 64));
    var installer = new PortableInstallerService(Path.Combine(area.Path, "data"));
    var installed = await installer.InstallZipAsync(app, release, archive, default);
    True(File.Exists(installed.ExecutablePath));
    Equal("pdfcraft.exe", Path.GetFileName(installed.ExecutablePath).ToLowerInvariant());
    Equal("0.3.0", installer.LoadState().Apps["printcraft"].Version);
}

static void Equal<T>(T expected, T actual)
{
    if (!EqualityComparer<T>.Default.Equals(expected, actual)) throw new InvalidOperationException($"Expected '{expected}', got '{actual}'.");
}

static void True(bool value)
{
    if (!value) throw new InvalidOperationException("Condition was false.");
}

static void Throws<T>(Action action) where T : Exception
{
    try { action(); }
    catch (T) { return; }
    throw new InvalidOperationException($"Expected {typeof(T).Name}.");
}

sealed class TempArea : IDisposable
{
    public string Path { get; } = System.IO.Path.Combine(AppContext.BaseDirectory, $"test-temp-{Guid.NewGuid():N}");
    public TempArea() => Directory.CreateDirectory(Path);
    public void Dispose() { try { Directory.Delete(Path, true); } catch { } }
}

sealed class StubHandler(Func<HttpRequestMessage, HttpResponseMessage> responder) : HttpMessageHandler
{
    protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken) =>
        Task.FromResult(responder(request));
}
