# Third-Party Notices

Vigla distributions include production Rust and JavaScript dependencies,
third-party fonts, and the adapted source listed below. The generated,
self-contained `THIRD_PARTY_NOTICES.txt` inventory retains the selected Rust
license texts collected by `cargo-about`, nested legal files from bundled Rust
source, required copyright, patent, and third-party notices, the exact installed
JavaScript license and notice texts, and the curated attributions for the
locked production dependency graphs. This curated notice supplements the
project's Apache-2.0 LICENSE file; each component remains governed by its own
license.

Where a published Rust crate omits its workspace-level license, Vigla retains
the exact file from the crate's recorded upstream revision and pins it by
SHA-256. Generic SPDX terms are never presented with `<year>`, `<owner>`, or
similar placeholders as if they were an attribution: generation requires a
matching archive file, checksum-pinned upstream file, or a transparent package
metadata record. For the rare upstream package that publishes no copyright
line, the inventory says so and does not invent an owner.

## ONNX Runtime 1.24.2 (optional embeddings build)

License: `MIT`

Building the desktop app with `EMBEDDINGS=1` links the static ONNX Runtime
distribution selected by `ort-sys` for local embedding inference. The exact
upstream license and complete third-party notices retained from Microsoft's
`v1.24.2` tag are distributed with every build:

- [ONNX Runtime license](third_party_licenses/onnxruntime-1.24.2-LICENSE.txt)
- [ONNX Runtime third-party notices](third_party_licenses/onnxruntime-1.24.2-ThirdPartyNotices.txt)

## @fontsource-variable/inter 5.2.8

License: `OFL-1.1`

Copyright 2016 The Inter Project Authors (https://github.com/rsms/inter) Inter-Italic[opsz,wght].ttf: Copyright 2016 The Inter Project Authors (https://github.com/rsms/inter)

Full license text:
[third_party_licenses/inter-OFL-1.1.txt](third_party_licenses/inter-OFL-1.1.txt)

## @fontsource-variable/jetbrains-mono 5.2.8

License: `OFL-1.1`

Copyright 2020 The JetBrains Mono Project Authors (https://github.com/JetBrains/JetBrainsMono) JetBrainsMono-Italic[wght].ttf: Copyright 2020 The JetBrains Mono Project Authors (https://github.com/JetBrains/JetBrainsMono)

Full license text:
[third_party_licenses/jetbrains-mono-OFL-1.1.txt](third_party_licenses/jetbrains-mono-OFL-1.1.txt)

## Howard Hinnant date algorithms

License: `MIT`

The dependency-free Gregorian conversion in
`crates/event-schema/src/time.rs` is adapted from Howard Hinnant's
`civil_from_days` algorithm in the
[date project](https://github.com/HowardHinnant/date).

Copyright (c) 2015, 2016, 2017 Howard Hinnant  
Copyright (c) 2016 Adrian Colomitchi  
Copyright (c) 2017 Florian Dang  
Copyright (c) 2017 Paul Thompson  
Copyright (c) 2018, 2019 Tomasz Kamiński  
Copyright (c) 2019 Jiangang Zhuang

Full license text:
[third_party_licenses/howard-hinnant-date-MIT.txt](third_party_licenses/howard-hinnant-date-MIT.txt)
