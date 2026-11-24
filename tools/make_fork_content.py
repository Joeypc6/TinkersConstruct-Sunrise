"""Writes Tinkers' Construct's assets/ and data/ for the fork build (tconstruct-fork/), from the 1.20.1 jar.

It runs the upstream content generator (tconstruct-horizon/tools/make_content.py, unchanged) into a temporary .hmod
and changes what the fork reads differently:

- block entities are declared in code (`r.block_entity_type`), so no block borrows the beacon's or the furnace's
  block entity: the machines and stations copy stone (shaped or see-through) or an iron block (solid), with their own
  `facing`, and the controllers' light comes from `state_light` (13 while `active`, as TinkerSmeltery's
  `lightLevel(s -> s.getValue(ACTIVE) ? 13 : 0)`) instead of the furnace's `lit`;
- shape tables name their properties as a list (`"properties": [..]`);
- Tinkers' fluids become real fork fluids (`data/tconstruct/sunrise/fluid/*.json`): 70 world fluids with their own
  block and bucket (the upstream paper buckets go), the potion and powdered snow tank-only, so every tank, pipe and
  machine moves them through the FLUIDS capability;
- the W guide moves to `assets/tconstruct/ae2guide` (guide-fork reads it on the client);
- the tables the native mod compiles in (fluids, tools) become `src/gen_*.rs`;
- no `horizon-mod.toml` inside: the fork mod's own manifest is the only one.

Usage: python make_fork_content.py [jar]   (writes ../assets, ../data and ../src/gen_*.rs next to this folder)
"""

import json
import os
import re
import shutil
import sys
import tempfile
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
UPSTREAM_TOOLS = os.path.join(ROOT, "..", "tconstruct-horizon", "tools")
sys.path.insert(0, UPSTREAM_TOOLS)
sys.path.insert(0, HERE)
# Upstream paused its effects (the cheese block's speed and jump factors among them); the fork has them (the cheese
# block's BlockPart in `src/gadgets.rs`), so the content is generated as with them: the cheese is a plain block.
import paused  # noqa: E402
paused.PAUSED.discard("effects")
import make_content  # noqa: E402

JAR = os.path.join(ROOT, "..", "TConstruct-1.20.1-3.12.1.231.jar")
ID = "tconstruct"
TINKER_FLUIDS = r"Z:\AE2Converter-build\ref\decomp\tconstruct\src\slimeknights\tconstruct\fluids\TinkerFluids.java"

# Blocks that occlude as full opaque cubes once off the beacon (SearedBlock's solid props: the drains, ducts and
# chutes); every other former beacon block is see-through or shaped (tanks, gauges, tables, stations).
SOLID = ("_drain", "_duct", "_chute")
HORIZONTAL = {"type": "enum", "values": ["north", "south", "west", "east"]}
# The tanks and lanterns whose light is their fluid's (a `light` state 0..15).
LIT_TANKS = {f"{m}_{k}" for m in ("seared", "scorched") for k in ("fuel_tank", "fuel_gauge", "ingot_tank", "ingot_gauge", "lantern")}
# CastingTankBlock: `light` from its tank's fluid too (SearedTankBlock.LIGHT).
LIT_TANKS.add("seared_casting_tank")


def fix_shapes(decl):
    for k in ("shapes", "collision_shapes"):
        if k in decl and "property" in decl[k]:
            decl[k]["properties"] = [decl[k].pop("property")]


def fix_block(name, decl):
    fix_shapes(decl)
    props = decl.setdefault("properties", {})
    t = decl.get("template")
    if t == "minecraft:furnace":
        # The lit controllers (smeltery_data.LIT_CONTROLLERS): the furnace's facing and lit become our own facing and
        # a per-state light on `active`.
        decl["template"] = "minecraft:iron_block" if props.get("occludes", True) else "minecraft:stone"
        sp = decl.setdefault("state_properties", {})
        sp = {"facing": HORIZONTAL, **sp}
        decl["state_properties"] = sp
        decl.setdefault("default_state", {})["facing"] = "north"
        props.pop("light", None)
        decl["state_light"] = {"properties": ["active"], "values": {"true": 13, "false": 0}, "default": 0}
    elif t == "minecraft:beacon":
        solid = name.endswith(SOLID) and "shapes" not in decl
        decl["template"] = "minecraft:iron_block" if solid else "minecraft:stone"
        if not solid:
            props["occludes"] = False
    if name in LIT_TANKS:
        # SearedTankBlock.LIGHT / SearedLanternBlock: the light of the fluid inside (FluidType.getLightLevel), kept by the
        # tank's block entity; `state_light` makes it the block's light.
        decl.setdefault("state_properties", {})["light"] = {"type": "enum", "values": [str(i) for i in range(16)]}
        decl.setdefault("default_state", {})["light"] = "0"
        decl["state_light"] = {"properties": ["light"], "values": {str(i): i for i in range(16)}, "default": 0}
        props.pop("light", None)
    if props.get("light") == 0:
        props.pop("light")
    if not props:
        decl.pop("properties")
    return decl


def fluid_physics():
    """Each fluid's FluidBuilder tick rate (TinkerFluids: Mantle's FluidBuilder default 5; fiery liquid 30)."""
    out = {}
    try:
        src = open(TINKER_FLUIDS, encoding="utf-8").read()
    except OSError:
        return out
    for chunk in src.split("public static final")[1:]:
        m = re.search(r'register(?:Metal|Gem|Stone|Glass|Gas|Fluid)?\("([a-z_]+)"\)', chunk)
        if not m:
            continue
        rate = re.search(r"\.tickRate\((\d+)\)", chunk)
        out[m.group(1)] = int(rate.group(1)) if rate else 5
    return out


PB_BUTTONS = 40  # the part builder's pattern buttons sent to its screen (src/tools/station.rs PB_BUTTONS)
WT_DATA = 6 + 2 * 24  # the worktable's data slots (src/tools/modrecipe.rs WT_HEAD + 2 * WT_BUTTONS)


def stations(files):
    """Stage 3's menus: the crafting station's 3x3 grid crafts as vanilla's crafting table (the menu's `crafting`;
    its result is the menu's, not a block slot); the part builder's buttons reach its screen in data slots."""
    menu = f"data/{ID}/horizon/menu/"
    cs = json.loads(files[menu + "crafting_station.json"])
    cs["results"] = []
    cs["crafting"] = {"grid": 0, "size": 3, "result": 9}
    files[menu + "crafting_station.json"] = json.dumps(cs, indent=1).encode()
    pb = json.loads(files[menu + "part_builder.json"])
    pb["data_slots"] = 2 + PB_BUTTONS
    files[menu + "part_builder.json"] = json.dumps(pb, indent=1).encode()
    # Stage 3C: the tinker station's and anvils' error (code and three arguments: src/tools/modrecipe.rs Err), the
    # worktable's selection, recipe, message and buttons (WT_HEAD + 2 x WT_BUTTONS).
    for name, slots in (("tinker_station", 4), ("tinkers_anvil", 4), ("modifier_worktable", WT_DATA)):
        m = json.loads(files[menu + name + ".json"])
        m["data_slots"] = slots
        files[menu + name + ".json"] = json.dumps(m, indent=1).encode()


def bare_tools(files):
    """A tool or part with no material for a layer (a bare `/give tconstruct:pickaxe`, the guide's and hotbar's plain
    stacks) draws that layer's base sprite untinted, as Java's ToolModel/MaterialModel do for a missing material
    (getMaterialSprite falls back to the part's base texture). The upstream models fall back to the near-black
    `tconstruct:unknown` sprite instead: point each such fallback at the base model where one exists."""
    pre, changed = f"assets/{ID}/items/", 0

    def base(model):
        if not isinstance(model, str) or not model.endswith("_tconstruct_unknown"):
            return None
        b = model[: -len("_tconstruct_unknown")]
        ns, path = b.split(":", 1)
        if f"assets/{ns}/models/{path}.json" in files:
            return b
        # No base model yet: the unknown one with its sprites' base textures (each exists beside the material ones).
        src = files.get(f"assets/{ns}/models/{model.split(':', 1)[1]}.json")
        if not src:
            return None
        doc = json.loads(src)
        tex = {k: v.replace("_tconstruct_unknown", "") for k, v in (doc.get("textures") or {}).items()}
        if not all(f"assets/{t.split(':', 1)[0]}/textures/{t.split(':', 1)[1]}.png" in files for t in tex.values()
                   if ":" in t):
            return None
        doc["textures"] = tex
        files[f"assets/{ns}/models/{path}.json"] = json.dumps(doc, separators=(",", ":")).encode()
        return b

    def walk(node):
        nonlocal changed
        if isinstance(node, list):
            for x in node:
                walk(x)
        elif isinstance(node, dict):
            fb = node.get("fallback")
            if isinstance(fb, dict) and fb.get("type") == "minecraft:model" and (b := base(fb.get("model"))):
                fb["model"] = b
                changed += 1
            for v in node.values():
                walk(v)

    for name in [n for n in files if n.startswith(pre) and n.endswith(".json")]:
        doc = json.loads(files[name])
        before = changed
        walk(doc)
        if changed != before:
            files[name] = json.dumps(doc, separators=(",", ":")).encode()
    print("bare tool layers:", changed)


def retint(files):
    """Java tints a material's part layer by its render colour where the material has no sprite of its own
    (MaterialRenderInfo.vertexColor). The upstream models carry that colour as a constant; here it goes through the
    `tconstruct:material` tint source, which reads the stack's material at run time (src/tools/tool.rs): `index` -1
    is a part's `Material`, i >= 0 a tool's material i. Returns the colour table the source reads."""
    colors = {}
    pre = f"assets/{ID}/tinkering/materials/"
    for name in files:
        if name.startswith(pre) and name.endswith(".json"):
            path = name[len(pre):-5]
            key = f"{ID}:" + (path.replace("/", "#", 1) if "/" in path else path)
            try:
                c = json.loads(files[name].decode("utf-8-sig")).get("color")
            except ValueError:
                c = None
            if isinstance(c, str):
                v = int(c, 16)
                colors[key] = v | 0xFF000000 if len(c) <= 6 else v
    changed = 0

    def walk(node, ctx):
        nonlocal changed
        if isinstance(node, list):
            for x in node:
                walk(x, ctx)
            return
        if not isinstance(node, dict):
            return
        if node.get("type") == "minecraft:select":
            if node.get("property") == "minecraft:custom_model_data":
                ctx = node.get("index", 0)
            elif node.get("property") == "minecraft:component" and node.get("component") == "minecraft:custom_data":
                ctx = -1
        tints = node.get("tints")
        if ctx is not None and isinstance(tints, list):
            for i, t in enumerate(tints):
                if isinstance(t, dict) and t.get("type") == "minecraft:constant":
                    tints[i] = {"type": f"{ID}:material", "index": ctx, "default": t.get("value", 0xFFFFFF)}
                    changed += 1
        for k, v in node.items():
            if k != "tints":
                walk(v, ctx)

    pre = f"assets/{ID}/items/"
    for name in [n for n in files if n.startswith(pre) and n.endswith(".json")]:
        doc = json.loads(files[name])
        before = changed
        walk(doc, None)
        if changed != before:
            files[name] = json.dumps(doc, separators=(",", ":")).encode()
    print("material tints:", changed, "; colours:", len(colors))
    return colors


def nbt_read(data):
    """A gzip-free NBT reader: (name, (type, value)); compounds as dicts of (type, value), lists as (elem_type, [values])."""
    import struct
    pos = 0
    def take(n):
        nonlocal pos
        pos += n
        return data[pos - n:pos]
    def string():
        n = struct.unpack(">H", take(2))[0]
        return take(n).decode("utf-8", "surrogatepass")
    def payload(t):
        if t == 1: return struct.unpack(">b", take(1))[0]
        if t == 2: return struct.unpack(">h", take(2))[0]
        if t == 3: return struct.unpack(">i", take(4))[0]
        if t == 4: return struct.unpack(">q", take(8))[0]
        if t == 5: return struct.unpack(">f", take(4))[0]
        if t == 6: return struct.unpack(">d", take(8))[0]
        if t == 7: n = struct.unpack(">i", take(4))[0]; return take(n)
        if t == 8: return string()
        if t == 9:
            et = take(1)[0]; n = struct.unpack(">i", take(4))[0]
            return (et, [payload(et) for _ in range(n)])
        if t == 10:
            out = {}
            while True:
                tt = take(1)[0]
                if tt == 0: return out
                k = string(); out[k] = (tt, payload(tt))
        if t == 11: n = struct.unpack(">i", take(4))[0]; return list(struct.unpack(f">{n}i", take(4 * n)))
        if t == 12: n = struct.unpack(">i", take(4))[0]; return list(struct.unpack(f">{n}q", take(8 * n)))
        raise ValueError(t)
    t = take(1)[0]
    name = string()
    return (name, (t, payload(t)))


def nbt_write(root):
    import struct
    out = bytearray()
    def string(s):
        b = s.encode("utf-8", "surrogatepass"); out.extend(struct.pack(">H", len(b))); out.extend(b)
    def payload(t, v):
        if t == 1: out.extend(struct.pack(">b", v))
        elif t == 2: out.extend(struct.pack(">h", v))
        elif t == 3: out.extend(struct.pack(">i", v))
        elif t == 4: out.extend(struct.pack(">q", v))
        elif t == 5: out.extend(struct.pack(">f", v))
        elif t == 6: out.extend(struct.pack(">d", v))
        elif t == 7: out.extend(struct.pack(">i", len(v))); out.extend(v)
        elif t == 8: string(v)
        elif t == 9:
            et, items = v; out.append(et); out.extend(struct.pack(">i", len(items)))
            for x in items: payload(et, x)
        elif t == 10:
            for k, (tt, vv) in v.items():
                out.append(tt); string(k); payload(tt, vv)
            out.append(0)
        elif t == 11: out.extend(struct.pack(">i", len(v))); out.extend(struct.pack(f">{len(v)}i", *v))
        elif t == 12: out.extend(struct.pack(">i", len(v))); out.extend(struct.pack(f">{len(v)}q", *v))
    name, (t, v) = root
    out.append(t); string(name); payload(t, v)
    return bytes(out)


def travelers_defaults(files):
    """Java's `tconstruct:crafting_shaped_materials` (the result built from whichever material items fill its
    `material_value` keys) has no fork form, so each traveler's piece is also a plain `minecraft:crafting_shaped` with
    Java's default materials fixed in the result (copper plating, leather; the shield wood and leather), built on the
    first inventory tick (README, stage 3D)."""
    pre = f"data/{ID}/recipe/tools/armor/travelers/"
    defaults = {"plating": ("minecraft:copper_ingot", f"{ID}:copper"), "cuirass": ("minecraft:leather", f"{ID}:leather")}
    for name in [n for n in files if n.startswith(pre) and n.endswith(".json")]:
        doc = json.loads(files[name])
        if doc.get("type") != f"{ID}:crafting_shaped_materials":
            continue
        shield = "shield" in name
        key, mats = {}, {}
        for k, v in doc["key"].items():
            first = v[0] if isinstance(v, list) else v
            if first.get("type") == f"{ID}:material_value":
                stat = json.dumps(first.get("material"))
                if shield:
                    item, mat = ("#minecraft:planks", f"{ID}:wood") if k == "c" else ("minecraft:leather", f"{ID}:leather")
                else:
                    item, mat = defaults["cuirass" if "cuirass" in stat else "plating"]
                key[k], mats[k] = item, mat
            elif "tag" in first:
                key[k] = "#" + first["tag"].replace("forge:", "c:")
            elif "item" in first:
                key[k] = first["item"]
        out = {"type": "minecraft:crafting_shaped", "category": "equipment", "pattern": doc["pattern"], "key": key,
               "result": {"id": doc["result"]["item"], "count": 1,
                          "components": {f"{ID}:materials": [mats[p] for p in doc.get("parts", "")]}}}
        files[name[:-5] + "_default.json"] = json.dumps(out, indent=1).encode()


GLASS_FACES = ("down", "up", "north", "south", "west", "east")


def slime_trees(files):
    """The ender trees and the blood and ichor fungi as features of the game's own types, so the saplings grow them
    through the game's sapling growth (`block_grow_feature` in `src/world/sapling.rs` names the feature):
    - the ender trees: Tinkers' `extra_root_variants` root placer is the mangrove root placer it extends (its root
      variants by the congealed slime grown into are left out); the `leave_vines` decorator and the hanging
      propagules (`attached_to_leaves` with the sapling's `age` / `hanging`, states the content's sapling hasn't) are
      left out (README, gaps);
    - the fungi: SlimeFungusFeature is HugeFungusFeature with a tag for its base; `minecraft:huge_fungus` takes one
      block, so there is a copy for each block of `tconstruct:slimy_soil` (`<fungus>_on_<soil>`)."""
    pre = f"data/{ID}/worldgen/feature/"
    for tree in ("ender_slime_tree", "ender_slime_tree_tall"):
        doc = json.loads(files[pre + tree + ".json"])
        doc["root_placer"]["type"] = "minecraft:mangrove_root_placer"
        doc["root_placer"].pop("root_variants", None)
        # 1.20's dirt_provider / force_dirt (TreeFeature.setDirtAt: dirt under the trunk unless it is dirt already)
        # is 26.3's below_trunk_provider.
        dirt = doc.pop("dirt_provider", {"id": "minecraft:dirt"})
        force = doc.pop("force_dirt", False)
        rule = {"then": dirt, "if_true": {"type": "minecraft:true"} if force else
                {"type": "minecraft:not", "predicate": {"type": "minecraft:matching_block_tag", "tag": "minecraft:dirt"}}}
        doc["below_trunk_provider"] = {"type": "minecraft:rule_based", "rules": [rule]}
        doc["decorators"] = [d for d in doc["decorators"] if d["type"] not in (f"{ID}:leave_vines", "minecraft:attached_to_leaves")]
        files[pre + tree + ".json"] = json.dumps(doc, indent=1).encode()
    soils = []
    for kind in ("earth", "sky", "ichor", "ender", "vanilla"):
        soils += json.loads(files[f"data/{ID}/tags/block/slimy_soil/{kind}.json"])["values"]
    n = 0
    for fungus in ("blood_slime_fungus", "ichor_slime_fungus"):
        doc = json.loads(files[pre + fungus + ".json"])
        for soil in soils:
            out = dict(doc, type="minecraft:huge_fungus", valid_base_block={"id": soil})
            out.pop("valid_base", None)
            files[pre + f"{fungus}_on_{soil.split(':')[1]}.json"] = json.dumps(out, indent=1).encode()
            n += 1
    print("slime trees as game features: 2 ender trees,", n, "fungus copies")


def world_stage4(files):
    """Stage 4: the world's content.
    - Clear stained glass: Mantle's `colored_block` model colours its quads by the block's GlassColor; here its faces
      get `tintindex` 0 and the block a constant tint source (src/world/tint.rs). The panes' templates are copied
      with the index (`template/pane_tinted/`).
    - The slime colormaps (SlimeColorizer: 256 x 256, RGB) as `src/gen_colormaps.bin`, read by the tint sources.
    - The islands' spawn overrides name Tinkers' slimes, which can't be declared (no slime entity template at
      514ecf9: README, gaps); those entries are left out so the structures load."""
    pre = f"assets/{ID}/models/block/clear_glass/"
    tinted = 0
    for name in [n for n in files if n.startswith(pre) and n.endswith(".json")]:
        rest = name[len(pre):]
        if "/" not in rest:
            continue
        doc = json.loads(files[name])
        if rest.endswith("/block.json"):
            tex = doc.get("textures", {}).get("all", f"{ID}:block/clear_stained_glass")
            doc = {"parent": "minecraft:block/block", "render_type": "minecraft:translucent",
                   "textures": {"particle": tex, "all": tex},
                   "elements": [{"from": [0, 0, 0], "to": [16, 16, 16], "faces": {
                       f: {"texture": "#all", "cullface": f, "tintindex": 0} for f in GLASS_FACES}}]}
        elif doc.get("parent", "").startswith(f"{ID}:block/template/pane/"):
            doc["parent"] = doc["parent"].replace("/template/pane/", "/template/pane_tinted/")
        else:
            continue
        files[name] = json.dumps(doc, indent=1).encode()
        tinted += 1
    tpl = f"assets/{ID}/models/block/template/pane/"
    for name in [n for n in files if n.startswith(tpl) and n.endswith(".json")]:
        doc = json.loads(files[name])
        for el in doc.get("elements", []):
            for face in el.get("faces", {}).values():
                face["tintindex"] = 0
        doc["render_type"] = "minecraft:translucent"
        files[name.replace("/template/pane/", "/template/pane_tinted/")] = json.dumps(doc, indent=1).encode()
    print("clear stained glass models tinted:", tinted)

    import struct, zlib
    def png_rgb(data):
        pos, idat, w = 8, b"", 0
        ihdr, plte = None, b""
        while pos < len(data):
            n = int.from_bytes(data[pos:pos + 4], "big")
            kind, body = data[pos + 4:pos + 8], data[pos + 8:pos + 8 + n]
            if kind == b"IHDR":
                ihdr = struct.unpack(">IIBBBBB", body)
            elif kind == b"PLTE":
                plte = body
            elif kind == b"IDAT":
                idat += body
            pos += 12 + n
        w, h, depth, ctype = ihdr[0], ihdr[1], ihdr[2], ihdr[3]
        assert depth == 8 and ctype in (2, 3, 6), (depth, ctype)
        bpp = {2: 3, 3: 1, 6: 4}[ctype]
        raw = zlib.decompress(idat)
        stride = w * bpp
        out, prev, i = bytearray(), bytearray(stride), 0
        for _ in range(h):
            f, line = raw[i], bytearray(raw[i + 1:i + 1 + stride])
            i += 1 + stride
            for x in range(stride):
                a = line[x - bpp] if x >= bpp else 0
                b = prev[x]
                c = prev[x - bpp] if x >= bpp else 0
                if f == 1: line[x] = (line[x] + a) & 255
                elif f == 2: line[x] = (line[x] + b) & 255
                elif f == 3: line[x] = (line[x] + (a + b) // 2) & 255
                elif f == 4:
                    pa, pb, pc = abs(b - c), abs(a - c), abs(a + b - 2 * c)
                    line[x] = (line[x] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
            prev = line
            for x in range(w):
                out += plte[line[x] * 3:line[x] * 3 + 3] if ctype == 3 else line[x * bpp:x * bpp + 3]
        return w, h, bytes(out)
    blob = b""
    for t in ("earth", "sky", "ichor", "ender", "blood"):
        w, h, rgb = png_rgb(files[f"assets/{ID}/textures/colormap/{t}_grass_color.png"])
        assert (w, h) == (256, 256), (t, w, h)
        blob += rgb
    # As base64 in a Rust source (the launcher's build stages only the sources), decoded once by src/world/tint.rs.
    import base64
    with open(os.path.join(ROOT, "src", "gen_colormaps.rs"), "w", encoding="utf-8", newline="\n") as f:
        f.write("//! SlimeColorizer's colormaps (earth, sky, ichor, ender, blood; 256 x 256 RGB each), base64.\n"
                "//! Generated by tools/make_fork_content.py: don't edit.\n\n")
        f.write('pub(crate) const COLORMAPS_B64: &str = "' + base64.b64encode(blob).decode() + '";\n')


    # The island templates' fluids by Java's block names (`<fluid>_fluid`): the fork names a world fluid's block after
    # the fluid, and an unknown palette entry refuses the whole template. Sky and ender slime are tank-only here (their
    # names are the slime blocks'), so their pools become water (README, gaps).
    fluid_blocks = {"tconstruct:earth_slime_fluid": "tconstruct:earth_slime", "tconstruct:magma_fluid": "tconstruct:magma",
                    "tconstruct:sky_slime_fluid": "minecraft:water", "tconstruct:ender_slime_fluid": "minecraft:water"}
    import gzip
    pre_t = f"data/{ID}/structure/islands/"
    renamed = 0
    for name in [n for n in files if n.startswith(pre_t) and n.endswith(".nbt")]:
        root = nbt_read(gzip.decompress(files[name]))
        top = root[1][1]
        palettes = [top["palette"][1][1]] if "palette" in top else [p for p in top["palettes"][1][1]]
        for palette in palettes:
            for entry in (palette[1] if isinstance(palette, tuple) else palette):
                old = entry["Name"][1]
                if old in fluid_blocks:
                    entry["Name"] = (8, fluid_blocks[old])
                    renamed += 1
        files[name] = gzip.compress(nbt_write(root), mtime=0)
    print("island template fluids renamed:", renamed)

    pre = f"data/{ID}/worldgen/structure/"
    for name in [n for n in files if n.startswith(pre)]:
        doc = json.loads(files[name])
        for cat in doc.get("spawn_overrides", {}).values():
            cat["spawns"] = [s for s in cat.get("spawns", []) if not s.get("type", "").startswith(f"{ID}:")]
        files[name] = json.dumps(doc, indent=1).encode()


def write_rust_const(path, name, text, doc):
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(f"//! {doc}\n//! Generated by tools/make_fork_content.py: don't edit.\n\n")
        f.write(f"pub(crate) const {name}: &str = r######\"{text}\"######;\n")


EGG_COLOURS = {"earth_slime": 0x01CD4E, "sky_slime": 0x01CBCD, "ender_slime": 0xAF4CF6, "terracube": 0xA9AFC0}
VANILLA_EGG = (r"Z:\AE2Converter-build\fork-home\versions\nightly-2026-11-11-514ecf9\.local\horizon-client\26.3"
               r"\assets\minecraft\textures\item\slime_spawn_egg.png")


def spawn_eggs(files):
    """26.3 has no item/template_spawn_egg (every egg has its own texture), so Java's tinted template draws as missing.
    Each Tinkers egg gets vanilla's slime egg recoloured with its slime type's colour, and a plain generated model."""
    import io
    from PIL import Image
    base = Image.open(VANILLA_EGG).convert("RGBA")
    px, (w, h) = base.load(), base.size
    for name, c in EGG_COLOURS.items():
        rgb = ((c >> 16) & 255, (c >> 8) & 255, c & 255)
        out = Image.new("RGBA", (w, h))
        o = out.load()
        for y in range(h):
            for x in range(w):
                r, g, b, a = px[x, y]
                lum = (0.3 * r + 0.59 * g + 0.11 * b) / 255 * 1.35
                o[x, y] = tuple(min(255, int(v * lum)) for v in rgb) + (a,)
        buf = io.BytesIO()
        out.save(buf, "PNG")
        files[f"assets/{ID}/textures/item/{name}_spawn_egg.png"] = buf.getvalue()
        files[f"assets/{ID}/models/item/{name}_spawn_egg.json"] = json.dumps(
            {"parent": "minecraft:item/generated", "textures": {"layer0": f"{ID}:item/{name}_spawn_egg"}}, indent=4).encode()


def vanilla_ore_block_tags(files):
    """NeoForge's `c:ores/<kind>` block tags for vanilla's ores (NeoForge ships them in its own data; Sunrise has the
    item tags but not these block ones). Lustrous and chrysophilite match the broken block against them."""
    pre = "data/c/tags/block/ores/"
    vanilla = {"iron": ["#minecraft:iron_ores"], "gold": ["#minecraft:gold_ores"], "copper": ["#minecraft:copper_ores"],
               "netherite_scrap": ["minecraft:ancient_debris"], "coal": ["#minecraft:coal_ores"],
               "diamond": ["#minecraft:diamond_ores"], "emerald": ["#minecraft:emerald_ores"],
               "lapis": ["#minecraft:lapis_ores"], "redstone": ["#minecraft:redstone_ores"],
               "quartz": ["minecraft:nether_quartz_ore"]}
    for kind, values in vanilla.items():
        name = f"{pre}{kind}.json"
        doc = json.loads(files[name]) if name in files else {"replace": False, "values": []}
        doc["values"] = values + [v for v in doc.get("values", []) if v not in values]
        files[name] = json.dumps(doc, indent=1).encode()
    parent = "data/c/tags/block/ores.json"
    doc = json.loads(files[parent]) if parent in files else {"replace": False, "values": []}
    kids = sorted(n[len(pre):-5] for n in files if n.startswith(pre) and n.endswith(".json"))
    doc["values"] = doc.get("values", []) + [f"#c:ores/{k}" for k in kids if f"#c:ores/{k}" not in doc.get("values", [])]
    files[parent] = json.dumps(doc, indent=1).encode()


def mantle_loot_types(files):
    """Sunrise names a global loot modifier type after the mod that registers it: Mantle's `mantle:add_entry` and
    `mantle:replace_item` are registered as `tconstruct:add_entry` and `tconstruct:replace_item` (src/tools/loot.rs)."""
    for name in [n for n in files if n.startswith(f"data/{ID}/loot_modifiers/") and n.endswith(".json")]:
        text = files[name]
        for kind in (b"add_entry", b"replace_item"):
            text = text.replace(b'"mantle:' + kind + b'"', b'"tconstruct:' + kind + b'"')
        files[name] = text


def main(jar):
    with tempfile.TemporaryDirectory() as tmp:
        out = os.path.join(tmp, "content.hmod")
        fluids_path = os.path.join(tmp, "tconstruct-fluids.json")
        make_content.main(jar, out, os.path.join(tmp, "report.json"), fluids_path)
        files = {}
        with zipfile.ZipFile(out) as z:
            for name in z.namelist():
                files[name] = z.read(name)
        fluids = json.load(open(fluids_path, encoding="utf-8"))
        tools_json = open(os.path.join(tmp, "tconstruct-tools.json"), encoding="utf-8").read()
        report = json.load(open(os.path.join(tmp, "report.json"), encoding="utf-8"))
    files.pop("horizon-mod.toml", None)

    prefix = f"data/{ID}/horizon/block/"
    for name in [n for n in files if n.startswith(prefix)]:
        block = name[len(prefix):-5]
        files[name] = json.dumps(fix_block(block, json.loads(files[name])), indent=1).encode()

    # The fluids: world fluids for those Java gives a block (`*_fluid`), tank-only for the rest.
    world = {b[:-len("_fluid")] for b in report["skipped"]["fluid_blocks"]}
    # The fork names a world fluid's block after the fluid (Java: `<fluid>_fluid`), so a fluid whose name a block or
    # item already has (the sky and ender slime blocks, seared and scorched stone, the meat soup bowl) stays tank-only,
    # with its bucket an item of its own (README, gaps).
    taken = set()
    for kind in ("block", "item"):
        prefix = f"data/{ID}/horizon/{kind}/"
        taken |= {n[len(prefix):-5] for n in files if n.startswith(prefix)}
    world -= taken
    physics = fluid_physics()
    for fluid, look in fluids["textures"].items():
        path = fluid.split(":", 1)[1]
        decl = {"sprite": look["still"]}
        if look.get("color", 0xFFFFFFFF) != 0xFFFFFFFF:
            decl["tint"] = look["color"]
        if path in world:
            decl["world"] = {"tick_delay": physics.get(path, 5), "bucket": True}
            files.pop(f"data/{ID}/horizon/item/{path}_bucket.json", None)
        files[f"data/{ID}/horizon/fluid/{path}.json"] = json.dumps(decl, indent=1).encode()

    # The guide is read by the client (guide-fork): assets, not data.
    for name in [n for n in files if n.startswith(f"data/{ID}/ae2guide/")]:
        files[f"assets/{ID}/ae2guide/" + name[len(f"data/{ID}/ae2guide/"):]] = files.pop(name)
    # guide-fork resolves a scene's `src` against the page (no `<namespace>:` form): the scenes are named from the
    # guide's root instead, or no page found its scene.
    for name in [n for n in files if n.startswith(f"assets/{ID}/ae2guide/") and n.endswith(".md")]:
        files[name] = files[name].replace(f'src="{ID}:scenes/'.encode(), b'src="/scenes/')
    files[f"assets/{ID}/ae2guide/guide.json"] = json.dumps({
        "title": "tconstruct.guide.title", "start": "index.md",
        "recipe_types": ["tconstruct:melting", "tconstruct:casting_table", "tconstruct:casting_basin",
                         "tconstruct:alloy", "tconstruct:part_builder", "tconstruct:tool_building"],
        # How a <Recipe> box draws each type: the block that does it between the inputs and the outputs, as the
        # JEI categories show them (melter, table, basin, alloyer, part builder, molding).
        "recipe_layouts": {
            "tconstruct:melting": {"machine": "tconstruct:seared_melter", "inputs": ["ingredient"], "outputs": ["result", "byproducts"]},
            "tconstruct:casting_table": {"machine": "tconstruct:seared_table", "inputs": ["cast", "fluid"], "outputs": ["result"]},
            "tconstruct:casting_basin": {"machine": "tconstruct:seared_basin", "inputs": ["cast", "fluid"], "outputs": ["result"]},
            "tconstruct:ore_melting": {"machine": "tconstruct:foundry_controller", "inputs": ["ingredient"], "outputs": ["result", "byproducts"]},
            "tconstruct:alloy": {"machine": "tconstruct:scorched_alloyer", "inputs": ["inputs"], "outputs": ["result"]},
            "tconstruct:molding_table": {"machine": "tconstruct:seared_table", "inputs": ["material", "pattern"], "outputs": ["result"]},
            "tconstruct:molding_basin": {"machine": "tconstruct:seared_basin", "inputs": ["material", "pattern"], "outputs": ["result"]},
            **{f"tconstruct:{t}": {"machine": "tconstruct:modifier_worktable", "inputs": ["tools", "inputs"], "outputs": ["leftovers"]} for t in (
                "extract_modifier", "modifier_set_worktable", "modifier_sorting", "remove_modifier",
                "toggle_interaction", "enchantment_converting")},
            "tconstruct:item_part_builder": {"machine": "tconstruct:part_builder", "inputs": ["pattern_item"], "outputs": ["result"]},
        }}, indent=1).encode()

    # The loot tables in 26.3's syntax (tools/loot_tables.py).
    import loot_tables
    print("loot tables:", loot_tables.write(files))

    # Forge's parent tags (forge:ingots, forge:nuggets...) hold each child tag (#forge:ingots/iron...); the jar adds only
    # Tinkers' own items to them, so the `c:` parents would miss vanilla's (the gold casts and sand molding take any
    # ingot, nugget, gem...).
    pre = "data/c/tags/item/"
    for parent in ("ingots", "nuggets", "gems", "storage_blocks", "ores", "raw_materials", "dusts", "rods", "plates",
                   "gears", "coins", "wires"):
        kids = sorted({n[len(pre) + len(parent) + 1:-5] for n in files
                       if n.startswith(f"{pre}{parent}/") and n.endswith(".json") and "/" not in n[len(pre) + len(parent) + 1:]})
        if not kids:
            continue
        name = f"{pre}{parent}.json"
        doc = json.loads(files[name]) if name in files else {"replace": False, "values": []}
        values = doc.setdefault("values", [])
        added = [f"#c:{parent}/{k}" for k in kids if f"#c:{parent}/{k}" not in values]
        if added:
            values.extend(added)
            files[name] = json.dumps(doc, indent=1).encode()

    vanilla_ore_block_tags(files)
    mantle_loot_types(files)

    # The guide's title (guide-fork reads `tconstruct.guide.title`), in English.
    lang = f"assets/{ID}/lang/en_us.json"
    doc = json.loads(files[lang].decode("utf-8-sig"))
    doc["tconstruct.guide.title"] = "Tinkers' Construct"
    # Mantle's line for `/tconstruct generate hidden_fluids_tag` failing to write (Mantle's lang isn't shipped).
    doc.setdefault("command.mantle.modify_tag.write_error", "Error writing %s tag '%s'")
    files[lang] = json.dumps(doc, indent=1, ensure_ascii=False).encode("utf-8")

    stations(files)
    colors = retint(files)
    bare_tools(files)
    write_rust_const(os.path.join(ROOT, "src", "gen_colors.rs"), "MATERIAL_COLORS", json.dumps(colors, separators=(",", ":")),
                     "Each material's render colour (MaterialRenderInfo.vertexColor, `assets/tconstruct/tinkering/materials`).")

    extra = os.path.join(HERE, "extra")
    if os.path.isdir(extra):
        for dirpath, _, names in os.walk(extra):
            for n in names:
                full = os.path.join(dirpath, n)
                rel = os.path.relpath(full, extra).replace(os.sep, "/")
                files[rel] = open(full, "rb").read()

    # Stage 3D: the shields raise and block as vanilla's (BlocksAttacks), within half the definition's block angle.
    # The whole hit is stopped: a `base` block amount with factor 0 stopped only part of a smaller hit in game (README).
    for shield, angle in (("travelers_shield", 45.0), ("plate_shield", 90.0)):
        name = f"data/{ID}/horizon/item/{shield}.json"
        if name in files:
            doc = json.loads(files[name])
            doc.setdefault("components", {})["minecraft:blocks_attacks"] = {
                "block_delay_seconds": 0.25,
                "damage_reductions": [{"base": 0.0, "factor": 1.0, "horizontal_blocking_angle": angle}],
                "block_sound": "minecraft:item.shield.block",
                "disabled_sound": "minecraft:item.shield.break",
            }
            files[name] = json.dumps(doc, indent=1).encode()

    travelers_defaults(files)
    world_stage4(files)
    slime_trees(files)
    spawn_eggs(files)

    for tree in ("assets", "data"):
        shutil.rmtree(os.path.join(ROOT, tree), ignore_errors=True)
    for name, data in files.items():
        # Sunrise reads a content mod's blocks, items, menus and tabs from data/<id>/sunrise/ (Horizon's was horizon/).
        name = name.replace(f"data/{ID}/horizon/", f"data/{ID}/sunrise/", 1)
        path = os.path.join(ROOT, *name.split("/"))
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "wb") as f:
            f.write(data)
    fluids["world"] = sorted(f"{ID}:{w}" for w in world)
    # The still sprites' texture heights (their animation frames stacked), so a screen can draw a fluid's first frame
    # (the melter's tank, GuiUtil.renderFluidTank) with a blit of part of the texture.
    heights = {}
    for look in fluids["textures"].values():
        ns, path = look["still"].split(":", 1) if ":" in look["still"] else ("minecraft", look["still"])
        png = files.get(f"assets/{ns}/textures/{path}.png")
        if png and png[1:4] == b"PNG":
            heights[look["still"]] = int.from_bytes(png[20:24], "big")
    fluids["heights"] = heights
    write_rust_const(os.path.join(ROOT, "src", "gen_fluids.rs"), "FLUIDS_JSON", json.dumps(fluids, separators=(",", ":")),
                     "The fluid table (tconstruct-horizon/tools/smeltery_data.py): tags, textures, buckets, units, light.")
    # The fluid effects (FluidEffectManager, `tinkering/fluid_effects`): spilling's and the fluid tools' entity effects
    # read them (src/tools/tank.rs).
    pre = f"data/{ID}/tinkering/fluid_effects/"
    effects = {n[len(pre):-5]: json.loads(d) for n, d in files.items() if n.startswith(pre) and n.endswith(".json")}
    write_rust_const(os.path.join(ROOT, "src", "gen_fluid_effects.rs"), "FLUID_EFFECTS_JSON", json.dumps(effects, separators=(",", ":")),
                     "Tinkers' fluid effects (`tinkering/fluid_effects/*.json`), by file name.")
    write_rust_const(os.path.join(ROOT, "src", "gen_tools.rs"), "TOOLS_JSON", tools_json,
                     "The material and tool table (tconstruct-horizon/tools/tool_data.py).")
    print(len(files), "files written under", ROOT, ";", len(world), "world fluids")


def commands_menu():
    """The `/tconstruct` commands menu's pages (`../../tools/commands-menu/tconstruct.json`), written into assets/ after
    the wipe."""
    import importlib.util
    path = os.path.join(ROOT, "..", "tools", "commands-menu", "pages.py")
    spec = importlib.util.spec_from_file_location("commands_menu_pages", path)
    pages = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(pages)
    pages.install(ROOT, "tconstruct")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else JAR)
    commands_menu()
