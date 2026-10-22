# pollen-skills

Python bindings for [pollen](https://github.com/groupbees/pollen), which
deploys Agent Skills declared in a `pollen.yaml` from git repositories and
local directories. Every `pollen` command is a function that returns typed
results instead of printing them; the engine is the same Rust code as the CLI,
compiled into the package.

```sh
pip install pollen-skills
```

```python
import pollen

report = pollen.update(config="pollen.yaml")
for summary in report.summaries:
    for change in summary.changes:
        print(summary.target, change.action, change.name)
```

`git` must be on `PATH`, as for the CLI. The full API reference is in the
[documentation](https://groupbees.github.io/pollen/guides/python/).
