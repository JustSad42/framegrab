"""Builds assets/phosphor-min.ttf: the Phosphor icon font cut down to the glyphs the code uses.

Run after adding an icon:  python scripts/subset_icons.py
Needs fontTools (pip install fonttools). The full font stays in assets/phosphor.ttf for lookups.
"""
import pathlib
import re

from fontTools import subset

root = pathlib.Path(__file__).resolve().parent.parent
used = set()
for f in (root / "src").rglob("*.rs"):
    for m in re.finditer(r"\\u\{(E[0-9A-Fa-f]{3})\}", f.read_text(encoding="utf8")):
        used.add(int(m.group(1), 16))

opts = subset.Options()
opts.layout_features = []
opts.name_IDs = [1, 2]
opts.notdef_outline = False
opts.drop_tables += ["GSUB", "GPOS", "DSIG"]
font = subset.load_font(str(root / "assets" / "phosphor.ttf"), opts)
s = subset.Subsetter(opts)
s.populate(unicodes=sorted(used))
s.subset(font)
out = root / "assets" / "phosphor-min.ttf"
subset.save_font(font, str(out), opts)
print(f"{len(used)} icons -> {out.name} ({out.stat().st_size // 1024} KB)")
