using System.Text.Json;
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
        return manifest;
    }
}
