# Brand assets

| File | Use |
|---|---|
| `wordmark-light.svg`, `wordmark-dark.svg` | The name, for light and dark backgrounds |
| `banner-light.svg`, `banner-dark.svg` | The name and tagline, as at the top of the README |
| `logo.svg` | The full stop alone: favicons and avatars |
| `social-preview.svg`, `social-preview.png` | 1280 × 640 repository preview for GitHub and link cards |
| `demo.svg` | Animated demo of real completions |

The wordmark is `complet` in ink (`#111113`, or paper `#F6F5F1` on dark), then `r` and a square full stop in
orange (`#F25C05`). Use the orange for that and little else; links on light backgrounds take the darker
`#C2410C`, which reads at body size. Text is converted to outlines from
[Space Grotesk](https://github.com/floriankarsten/space-grotesk) (the name) and
[Source Sans 3](https://github.com/adobe-fonts/source-sans) (the tagline and demo), both under the SIL Open
Font License, so the files need no fonts.

Regenerate everything with `python assets/src/build.py <font dir>`, where the directory holds the variable
fonts `SpaceGrotesk.ttf` and `SourceSans3.ttf`, and the demo with
`python assets/src/demo_data.py && python assets/src/demo.py <font dir>`.
