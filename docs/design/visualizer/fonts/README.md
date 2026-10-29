# Fonts

The mockups self-host their type, as `bracken view` will: no CDN, no network.

| File | Face | Source | SHA-256 of the source file |
|---|---|---|---|
| `alegreya.woff2` | Alegreya, variable weight 400–900 | `google/fonts`, `ofl/alegreya/Alegreya[wght].ttf` | `ba5564634b93a8f8ba57b48cd4f1ae7417d2b4656fbac779028679b00de3cf12` |
| `alegreya-italic.woff2` | Alegreya Italic, variable weight | `ofl/alegreya/Alegreya-Italic[wght].ttf` | `fa915eec76227935dc5fb678953c94b71287c360928013cfdb441dfe52f5a391` |
| `alegreya-sans-sc-medium.woff2` | Alegreya Sans SC Medium | `ofl/alegreyasanssc/AlegreyaSansSC-Medium.ttf` | `532c7dc4bc0fe8109d2c5f17829d9985597aacb86e8001023e045d4ee00f356a` |

Both are by Huerta Tipográfica (Juan Pablo del Peral) under the SIL Open Font License 1.1 ([OFL.txt](OFL.txt)), which allows
bundling and subsetting; the subsets keep the licence and aren't sold on their own. Supply-chain check: a font is data,
not code; the sources are Google Fonts' own repository, pinned by hash above.

Subset to Latin-1 plus punctuation and arrows (about 107 KB for the three), with fontTools:

```sh
pyftsubset <source>.ttf --unicodes="U+0020-007E,U+00A0-00FF,U+2010-2027,U+2030-203A,U+2190-2193,U+2212,U+00D7" \
  --flavor=woff2 --layout-features='kern,liga,onum,lnum,tnum,smcp,c2sc' --output-file=<name>.woff2
```

Text outside the subset (other scripts, emoji) falls back to the system's fonts, glyph by glyph.
