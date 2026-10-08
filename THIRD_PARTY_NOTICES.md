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
- [PrintCraft](https://github.com/storytold/printcraft)

Each application is separately licensed by its copyright holders. The upstream
license and notices shipped with each downloaded build govern that application.
The manager's Personal Use License does not relicense or grant rights to those
applications, names, logos, or other trademarks.

The seven application icons are copied from the official upstream repositories.
Their exact source paths, snapshot commits, and verbatim upstream licence files
are preserved in [`assets/icons`](assets/icons/README.md). They are used only to
identify their corresponding applications.

This project uses the Rust [egui/eframe](https://github.com/emilk/egui) GUI stack,
distributed under MIT or Apache-2.0, plus the Rust crates recorded in
`Cargo.lock`. Those dependencies remain under their respective licences.
