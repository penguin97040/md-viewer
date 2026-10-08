# Third-party components

MD Viewer bundles or links the following open-source software.

| Component | Version | Licence | Use |
|---|---|---|---|
| [Mermaid](https://github.com/mermaid-js/mermaid) | 12.1.0 | MIT | Diagrams (`frontend/vendor/mermaid.min.js.gz`) |
| [DOMPurify](https://github.com/cure53/DOMPurify) | 3.4.16 | Apache-2.0 OR MPL-2.0 | HTML sanitising inside Mermaid |
| [KaTeX](https://github.com/KaTeX/KaTeX) | 0.19.0 | MIT | Maths (`frontend/vendor/katex/`) |
| [Wails](https://github.com/wailsapp/wails) | 2.16.0 | MIT | Desktop shell |
| [goldmark](https://github.com/yuin/goldmark) | 1.8 | MIT | Markdown parser |
| [hugo-goldmark-extensions/passthrough](https://github.com/gohugoio/hugo-goldmark-extensions) | 0.5 | Apache-2.0 | Maths delimiters |
| [Chroma](https://github.com/alecthomas/chroma) | 2 | MIT | Syntax highlighting |
| [bluemonday](https://github.com/microcosm-cc/bluemonday) | 1 | BSD-3-Clause | HTML sanitising |
| [fsnotify](https://github.com/fsnotify/fsnotify) | 1 | BSD-3-Clause | Live reload |

Licence texts for the bundled frontend libraries are kept next to them in
`frontend/vendor/`. Go module licences are in each module's source.

Mermaid is rebuilt from its core entrypoint with DOMPurify 3.4.16 and KaTeX
0.19.0, using esbuild 0.28.2 (MIT). The same KaTeX version is used for diagram
labels and standalone maths. The maintenance dependency graph, including
integrity hashes and overrides, is locked in `scripts/vendor/package-lock.json`.
`frontend/vendor/components.json` records versions and bundle hashes;
`frontend/vendor/LICENSE-mermaid-dependencies.txt` contains the licence texts
for all packages included in the Mermaid bundle. esbuild is a maintenance tool
and is not shipped as executable code.
