# Brand assets

| File | Use |
|---|---|
| `logo.svg` | Icon on its own: favicons, avatars, app tiles |
| `wordmark-light.svg`, `wordmark-dark.svg` | The name alone, for light and dark backgrounds |
| `lockup-light.svg`, `lockup-dark.svg` | Icon and name side by side, where a header or slide needs both |
| `banner-light.svg`, `banner-dark.svg` | Icon, name and tagline, as at the top of the README |
| `social-preview.svg`, `social-preview.png` | 1280 × 640 repository preview for GitHub and link cards |
| `demo.svg` | Animated demo of real completions |

The wordmark shows the name being completed: `str` typed, a cursor, and `ato` as the suggestion. Keep the
cursor cyan (`#22D3EE`) and the completion lighter than the typed part. Text is converted to outlines from
[Inter](https://rsms.me/inter/) (SIL Open Font License), so the files need no fonts.

Regenerate everything with `python assets/src/build.py <Inter ttf directory>`, and the demo with
`python assets/src/demo_data.py && python assets/src/demo.py <Inter ttf directory>`.
