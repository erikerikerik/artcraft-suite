# Third-party notices

ArtCraft Suite Manager is an independent community project. It is not an official
ArtCraft or storytold product and is not endorsed by the upstream maintainers.

The manager downloads unmodified release artifacts from these official upstream
repositories:

- [PhotoCraft](https://github.com/storytold/photocraft)
- [VectorCraft](https://github.com/storytold/vectorcraft)
- [DesignCraft](https://github.com/storytold/designcraft)
- [FilmCraft](https://github.com/storytold/filmcraft)
- [EffectCraft](https://github.com/storytold/effectcraft)
- [LightCraft](https://github.com/storytold/lightcraft)
- [PrintCraft](https://github.com/storytold/pdfcraft) (published as PDFCraft)

The macOS manager also installs:

- [WordCraft](https://github.com/storytold/wordcraft)
- [GridCraft](https://github.com/storytold/gridcraft)
- [DeckCraft](https://github.com/storytold/deckcraft)
- [SoundCraft](https://github.com/storytold/soundcraft)
- [CADCraft](https://github.com/storytold/cadcraft)

These five repositories state that their code is licensed under the MIT License or
the Apache License 2.0, at your option, and that the ArtCraft names and marks in
their `docs/brand/` folders are not covered by that licence.

Each application is separately licensed by its copyright holders. At the time
this project was prepared, each repository identified its source license as
Apache-2.0. The upstream license and notices shipped with each downloaded build
govern that application. The manager's Apache-2.0 license does not relicense
or grant rights to those applications, names, logos, or other trademarks.

The macOS manager embeds each application's official 256×256 app icon, unmodified,
to identify it in the list. Their sources, snapshot commits and upstream licence
texts are in [`assets/icons/`](assets/icons/README.md). The manager reads the macOS
system font from the computer it runs on; no font files are bundled.

The macOS manager is built with [egui and eframe](https://github.com/emilk/egui)
(MIT OR Apache-2.0) and other Rust crates under permissive licences; the exact
versions are listed in `Cargo.lock`, which is shipped inside the app bundle.

This project uses [Avalonia UI](https://github.com/AvaloniaUI/Avalonia),
which is distributed under the MIT License, along with its transitive runtime
dependencies. NuGet package license metadata is included in release builds by
their respective packages.
