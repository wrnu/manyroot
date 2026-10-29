# Visualizer mockups

These are static mockups for the Ground visualizer's design spike. The design itself is in
[../visualizer.md](../visualizer.md). Open the pages straight from disk; they need no server and no network.

- [loom.html](loom.html): the Loom, with agents as roots and contributions woven to their causal parents.
- [root.html](root.html): the Root view, one claim's life.
- [language.html](language.html): the visual language sheet.

The data in `sample-data.js` comes from a throwaway Ground, built by `sample/`, and is synthetic. To
rebuild it:

```sh
cargo build -p bracken
dir=$(docs/design/visualizer/sample/make-sample-ground.sh)
python3 -I docs/design/visualizer/sample/export-sample.py "$dir"
```
