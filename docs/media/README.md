# Public media

These assets support the README, GitHub Pages site, independent reviews, and
release coverage. Product screenshots and motion show the real UI driven by
hand-authored, deterministic canonical events — scripted, not captured from a
real session. They contain no credentials, private prompts, user paths, or
vendor traffic. Do not caption them as recordings of real agent runs.

Unless an asset says otherwise, it is distributed under the repository's
[Apache 2.0 license](../../LICENSE).

## Product captures

Run `scripts/capture-readme-media.cjs` against the local end-to-end mock surface
to regenerate `ops-room.png`, `plan-review.png`, `mission-inbox.png`, the two
1280×800 WebP derivatives — `vigla-demo-poster.webp` (from the Operations Room
capture) and `mission-inbox.webp`. There is no `plan-review` WebP. Run
`scripts/capture-web-demo.cjs` to regenerate `vigla-demo.webp`.

## Editorial identity

`social-preview.svg` is the hand-authored source for the README masthead and
repository link card. It uses only geometric marks, system type, and the
project's worktree → audit → merge model; it is not generated from a prompt.
Render the upload-ready PNG with:

```sh
rsvg-convert -w 1280 -h 640 docs/media/social-preview.svg \
  | magick png:- -strip -define png:compression-level=9 docs/media/social-preview.png
```

The PNG must remain 1280×640 and under GitHub's 1 MB social-preview limit.

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

The illustration is retained for provenance and is not used by the README or
landing page. Product status and commitments come only from the linked roadmap
text.
