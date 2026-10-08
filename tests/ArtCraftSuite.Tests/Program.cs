using System.IO.Compression;
using System.Net;
using System.Net.Http.Headers;
using System.Security.Cryptography;
using System.Text.Json;
using ArtCraftSuite.Models;
using ArtCraftSuite.Services;
using ArtCraftSuite.ViewModels;

var tests = new (string Name, Func<Task> Run)[]
{
    ("manifest maps PrintCraft to PDFCraft packages", TestManifestAlias),
    ("manifest rejects unsafe application ids", TestManifestValidation),
    ("release helpers select exact package and checksum", TestReleaseHelpers),
    ("stable channel falls back when GitHub API is rate limited", TestStableFallback),
    ("release cache honors ETags and works offline", TestReleaseCache),
    ("downloads resume partial files and verify SHA-256", TestDownloadResume),
    ("version comparison handles prereleases", TestVersions),
    ("ZIP extraction rejects traversal", TestTraversal),
    ("ZIP extraction rejects symbolic links", TestSymbolicLink),
    ("every manifest package layout locates its executable", TestAllPackageLayouts),
    ("legacy Rust install state migrates without false repair warnings", TestRustStateMigration),
    ("failed updates restore the previous app and state", TestUpdateRollback),
    ("diagnostic reports are structured and copy-safe", TestDiagnostics)
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

static Task TestManifestValidation()
{
    var unsafeApp = new AppManifest("../escape", "Bad", "Bad", "storytold/bad", null, "#fff",
        new Dictionary<string, string> { ["windows-x64"] = "^{id}-{version}\\.zip$" });
    Throws<InvalidDataException>(() => ManifestService.Validate(new SuiteManifest(1, [unsafeApp])));
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

static async Task TestReleaseCache()
{
    using var area = new TempArea();
    var hash = new string('d', 64);
    var json = $$"""[{"tag_name":"v1.2.3","draft":false,"prerelease":false,"assets":[{"name":"demo-1.2.3-windows-x64-portable.zip","browser_download_url":"https://github.com/a/b/demo.zip","size":12,"digest":"sha256:{{hash}}"}]}]""";
    var first = new GitHubReleaseService(new StubHandler(_ =>
    {
        var response = new HttpResponseMessage(HttpStatusCode.OK) { Content = new StringContent(json) };
        response.Headers.ETag = new EntityTagHeaderValue("\"release-v1\"");
        return response;
    }), Path.Combine(area.Path, "cache"));
    var app = DemoApp();
    Equal("1.2.3", (await first.ResolveAsync(app, "Stable", "windows-x64", false, default)).Version);

    var conditional = new GitHubReleaseService(new StubHandler(request =>
    {
        True(request.Headers.IfNoneMatch.Any(x => x.Tag == "\"release-v1\""));
        return new HttpResponseMessage(HttpStatusCode.NotModified);
    }), Path.Combine(area.Path, "cache"));
    Equal("1.2.3", (await conditional.ResolveAsync(app, "Stable", "windows-x64", false, default)).Version);

    var offline = new GitHubReleaseService(new ThrowingHandler(), Path.Combine(area.Path, "cache"));
    Equal("1.2.3", (await offline.ResolveAsync(app, "Stable", "windows-x64", false, default)).Version);
}

static async Task TestDownloadResume()
{
    using var area = new TempArea();
    var bytes = Enumerable.Range(0, 1000).Select(x => (byte)(x % 251)).ToArray();
    var destination = Path.Combine(area.Path, "download.partial");
    await File.WriteAllBytesAsync(destination, bytes[..317]);
    var handler = new StubHandler(request =>
    {
        Equal(317L, request.Headers.Range!.Ranges.Single().From!.Value);
        var content = new ByteArrayContent(bytes[317..]);
        content.Headers.ContentRange = new ContentRangeHeaderValue(317, 999, 1000);
        return new HttpResponseMessage(HttpStatusCode.PartialContent) { Content = content };
    });
    var hash = Convert.ToHexString(SHA256.HashData(bytes)).ToLowerInvariant();
    var release = new ResolvedRelease("1.0.0", "v1.0.0", new GitHubAsset("demo.zip", "https://github.com/a/b/demo.zip", bytes.Length, $"sha256:{hash}"), hash);
    await new GitHubReleaseService(handler, Path.Combine(area.Path, "cache")).DownloadVerifiedAsync(release, destination, null, default);
    True(bytes.SequenceEqual(await File.ReadAllBytesAsync(destination)));
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

static async Task TestAllPackageLayouts()
{
    using var area = new TempArea();
    var manifest = JsonSerializer.Deserialize<SuiteManifest>(File.ReadAllText(Path.Combine(AppContext.BaseDirectory, "manifest", "apps.json")),
        new JsonSerializerOptions { PropertyNameCaseInsensitive = true })!;
    var installer = new PortableInstallerService(Path.Combine(area.Path, "data"));
    foreach (var app in manifest.Apps)
    {
        var archive = Path.Combine(area.Path, $"{app.EffectivePackageId}.zip");
        using (var zip = ZipFile.Open(archive, ZipArchiveMode.Create))
        {
            var entry = zip.CreateEntry($"nested/{app.EffectivePackageId}.exe");
            await using var output = entry.Open();
            await output.WriteAsync(System.Text.Encoding.UTF8.GetBytes($"{app.Id} executable"));
        }
        var release = TestRelease("0.3.0", archive);
        var installed = await installer.InstallZipAsync(app, release, archive, default);
        True(File.Exists(installed.ExecutablePath));
        Equal($"{app.EffectivePackageId}.exe", Path.GetFileName(installed.ExecutablePath).ToLowerInvariant());
    }
    Equal("0.3.0", installer.LoadState().Apps["printcraft"].Version);
}

static async Task TestUpdateRollback()
{
    using var area = new TempArea();
    var root = Path.Combine(area.Path, "data");
    var app = DemoApp();
    var v1 = await CreateArchive(area.Path, "v1", app.EffectivePackageId);
    var v2 = await CreateArchive(area.Path, "v2", app.EffectivePackageId);
    await new PortableInstallerService(root).InstallZipAsync(app, TestRelease("1.0.0", v1), v1, default);
    var failing = new PortableInstallerService(root, phase => { if (phase == "AfterActivate") throw new IOException("simulated state failure"); });
    await ThrowsAsync<IOException>(() => failing.InstallZipAsync(app, TestRelease("2.0.0", v2), v2, default));
    var restored = new PortableInstallerService(root).LoadState().Apps[app.Id];
    Equal("1.0.0", restored.Version);
    Equal("v1", await File.ReadAllTextAsync(restored.ExecutablePath));
}

static Task TestRustStateMigration()
{
    using var area = new TempArea();
    var root = Path.Combine(area.Path, "data");
    var appDirectory = Path.Combine(root, "apps", "demo");
    Directory.CreateDirectory(appDirectory);
    var executable = Path.Combine(appDirectory, "demo.exe");
    File.WriteAllText(executable, "legacy executable");
    var legacy = JsonSerializer.Serialize(new
    {
        apps = new Dictionary<string, object>
        {
            ["demo"] = new { id = "demo", version = "0.2.1", launchPath = executable, sourceAsset = "demo.zip", sha256 = "abc", installedAtUnix = 1_700_000_000L }
        }
    });
    File.WriteAllText(Path.Combine(root, "state.json"), legacy);

    var installed = new PortableInstallerService(root).LoadState().Apps["demo"];
    Equal(executable, installed.ExecutablePath);
    Equal("0.2.1", installed.Version);
    True(File.ReadAllText(Path.Combine(root, "state.json")).Contains("ExecutablePath", StringComparison.Ordinal));
    return Task.CompletedTask;
}

static Task TestDiagnostics()
{
    using var area = new TempArea();
    var diagnostics = new DiagnosticService(Path.Combine(area.Path, "logs"));
    diagnostics.Info("test", "safe message");
    var report = diagnostics.BuildReport("Stable", "Ready", ["Demo: installed=no"]);
    True(File.Exists(diagnostics.LogPath));
    True(File.ReadAllLines(diagnostics.LogPath).All(line => JsonDocument.Parse(line).RootElement.GetProperty("timestamp").ValueKind == JsonValueKind.String));
    True(report.Contains("Version:", StringComparison.Ordinal));
    True(report.Contains("Demo: installed=no", StringComparison.Ordinal));
    return Task.CompletedTask;
}

static AppManifest DemoApp() => new("demo", "Demo", "Demo", "storytold/demo", null, "#fff",
    new Dictionary<string, string> { ["windows-x64"] = "^{id}-{version}-windows-x64-portable\\.zip$" });

static ResolvedRelease TestRelease(string version, string archive) =>
    new(version, $"v{version}", new GitHubAsset(Path.GetFileName(archive), "https://invalid/", 0, null), new string('0', 64));

static async Task<string> CreateArchive(string root, string version, string executable)
{
    var archive = Path.Combine(root, $"{version}.zip");
    using var zip = ZipFile.Open(archive, ZipArchiveMode.Create);
    var entry = zip.CreateEntry($"nested/{executable}.exe");
    await using var output = entry.Open();
    await output.WriteAsync(System.Text.Encoding.UTF8.GetBytes(version));
    return archive;
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

static async Task ThrowsAsync<T>(Func<Task> action) where T : Exception
{
    try { await action(); }
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

sealed class ThrowingHandler : HttpMessageHandler
{
    protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken) =>
        throw new HttpRequestException("offline");
}
