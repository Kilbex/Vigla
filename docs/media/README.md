# Public media

These assets support the README, GitHub Pages site, independent reviews, and
release coverage. Product screenshots and motion use fictional deterministic
events; they contain no credentials, private prompts, user paths, or vendor
traffic.

Unless an asset says otherwise, it is distributed under the repository's
[Apache 2.0 license](../../LICENSE).

## Product captures

Run `scripts/capture-readme-media.cjs` against the local end-to-end mock surface
to regenerate `ops-room.png`, `plan-review.png`, `mission-inbox.png`, their web
derivatives, and the 1280×640 social card. Run
`scripts/capture-web-demo.cjs` to regenerate `vigla-demo.webp`.

The social card is composed deterministically from the Operations Room capture,
so link previews show the real product rather than a conceptual mockup.

## Roadmap illustration

- Source: `roadmap-horizon.png` (1694×928, AI-generation provenance retained)
- README derivative: `roadmap-horizon.webp` (1536×842)
- Source document: [`ROADMAP.md`](../../ROADMAP.md), including the linked track
  specifications

Generation prompt:

> Visualize the public Vigla roadmap as an abstract night-watch operations map
> moving through three connected stages: present-day hardening and verified
> local builds; next-stage modular agent connections, reusable mission
> templates, and a guarded application-sandbox feasibility gate; long-horizon
> platform expansion, richer memory provenance, and transparent benchmarking.
> Use a deep near-black technical landscape, a luminous signal route, premium
> editorial 3D geometry, teal with restrained amber accents, and generous wide
> framing. Include no words, letters, numbers, logos, trademarks, brand marks,
> app icons, vendor symbols, people, robot mascots, watermarks, or fake UI.

The illustration is interpretive. Product status and commitments come only
from the linked roadmap text.
