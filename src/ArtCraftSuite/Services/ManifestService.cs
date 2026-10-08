using System.Text.Json;
using System.Text.RegularExpressions;
using ArtCraftSuite.Models;

namespace ArtCraftSuite.Services;

public static class ManifestService
{
    private static readonly JsonSerializerOptions JsonOptions = new() { PropertyNameCaseInsensitive = true };

    public static SuiteManifest Load()
    {
        var path = Path.Combine(AppContext.BaseDirectory, "manifest", "apps.json");
        if (!File.Exists(path)) throw new FileNotFoundException("The application manifest is missing.", path);
        var manifest = JsonSerializer.Deserialize<SuiteManifest>(File.ReadAllText(path), JsonOptions)
            ?? throw new InvalidDataException("The application manifest is invalid.");
        if (manifest.SchemaVersion != 1) throw new InvalidDataException($"Unsupported manifest schema {manifest.SchemaVersion}.");
        Validate(manifest);
        return manifest;
    }

    internal static void Validate(SuiteManifest manifest)
    {
        if (manifest.Apps.Count == 0) throw new InvalidDataException("The application manifest is empty.");
        var ids = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var app in manifest.Apps)
        {
            if (!Regex.IsMatch(app.Id, "^[a-z0-9-]+$", RegexOptions.CultureInvariant) || !ids.Add(app.Id))
                throw new InvalidDataException($"Invalid or duplicate application id: {app.Id}");
            if (!Regex.IsMatch(app.EffectivePackageId, "^[a-z0-9-]+$", RegexOptions.CultureInvariant))
                throw new InvalidDataException($"Invalid package id for {app.Id}.");
            if (!Regex.IsMatch(app.Repository, "^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$", RegexOptions.CultureInvariant))
                throw new InvalidDataException($"Invalid repository for {app.Id}.");
            foreach (var (platform, pattern) in app.AssetPatterns)
            {
                if (string.IsNullOrWhiteSpace(platform) || !pattern.StartsWith('^') || !pattern.EndsWith('$'))
                    throw new InvalidDataException($"Invalid package rule for {app.Id} on {platform}.");
                try { _ = new Regex(GitHubReleaseService.BuildAssetPattern(pattern, app.EffectivePackageId, "0.0.0"), RegexOptions.CultureInvariant); }
                catch (ArgumentException ex) { throw new InvalidDataException($"Invalid package rule for {app.Id} on {platform}.", ex); }
            }
        }
    }
}
