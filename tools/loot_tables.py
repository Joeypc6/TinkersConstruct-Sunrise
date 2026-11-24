"""Tinkers' 1.20.1 loot tables in 26.3's form, which the fork reads (the upstream port's tables were 1.20's, and the fork
read none of their conditions: `survives_explosion` failed, so no Tinkers block dropped anything).

26.3 changed the table syntax (compare the prepared `data/minecraft/loot_table/`):
- a pool's or entry's `conditions` list is one `condition` (a list of several is `minecraft:all_of`), and a condition's
  `condition` key is `type`;
- `functions` are `modifier`, and a function's `function` key is `type`;
- `match_tool`'s silk-touch predicate is the `minecraft:tool/can_silk_touch` condition, shears (by item or by Forge's
  `shears_dig` tool action) `minecraft:tool/can_shear`, a tag predicate `{"items": "#tag"}`;
- entity predicates are data components (`minecraft:entity_type`, `minecraft:flags`, `minecraft:type_specific/cube_mob`);
- `copy_name` is `copy_components` of `minecraft:custom_name`; `looting_enchant` is `enchanted_count_increase`.
Dropped, with what the native mod does instead: `copy_nbt` (the tanks' fluid: `block_drops` in src/smeltery) and
Mantle's `fill_retextured_block` (retextured tables keep their default look: README, gaps).
"""

import json

SILK = {"enchantments": [{"enchantment": "minecraft:silk_touch", "levels": {"min": 1}}]}


def condition(c):
    if isinstance(c, list):
        out = [condition(x) for x in c]
        return out[0] if len(out) == 1 else {"type": "minecraft:all_of", "terms": out}
    c = dict(c)
    kind = c.pop("condition", None) or c.pop("type", None)
    if kind == "minecraft:match_tool":
        p = c.get("predicate", {})
        if p == SILK:
            return "minecraft:tool/can_silk_touch"
        if p.get("items") == ["minecraft:shears"]:
            return "minecraft:tool/can_shear"
        if "tag" in p:
            return {"type": kind, "predicate": {"items": "#" + p["tag"].replace("forge:", "c:")}}
        if "items" in p:
            items = p["items"]
            return {"type": kind, "predicate": {"items": items[0] if len(items) == 1 else items}}
        return {"type": kind, **c}
    if kind == "forge:can_tool_perform_action" and c.get("action") == "shears_dig":
        return "minecraft:tool/can_shear"
    if kind in ("minecraft:any_of", "minecraft:all_of"):
        return {"type": kind, "terms": [condition(t) for t in c.get("terms", [])]}
    if kind == "minecraft:inverted":
        return {"type": kind, "term": condition(c["term"])}
    if kind == "minecraft:entity_properties":
        return {"type": kind, "entity": c.get("entity", "this"), "predicate": entity_predicate(c.get("predicate", {}))}
    if kind == "minecraft:block_state_property":
        # 26.3 renamed it: match_block, with `blocks` and `state` (Sunrise ed3ac19 refuses the old name).
        out = {"type": "minecraft:match_block", "blocks": c.get("block")}
        if "properties" in c:
            out["state"] = c["properties"]
        return out
    if kind == "minecraft:damage_source_properties":
        p = dict(c.get("predicate", {}))
        if "source_entity" in p:
            p["source_entity"] = entity_predicate(p["source_entity"])
        return {"type": kind, "predicate": p}
    return {"type": kind, **c}


def entity_predicate(p):
    out = {}
    for k, v in p.items():
        if k == "type":
            out["minecraft:entity_type"] = v
        elif k == "type_specific" and v.get("type") == "slime":
            out["minecraft:type_specific/cube_mob"] = {kk: vv for kk, vv in v.items() if kk != "type"}
        elif k in ("flags", "nbt", "equipment", "location", "effects"):
            out[f"minecraft:{k}"] = v
        else:
            out[k] = v
    return out


def function(f):
    f = dict(f)
    kind = f.pop("function")
    conds = f.pop("conditions", None)
    if kind == "minecraft:copy_name":
        out = {"type": "minecraft:copy_components", "source": "block_entity", "include": ["minecraft:custom_name"]}
    elif kind in ("minecraft:copy_nbt", "mantle:fill_retextured_block"):
        return None
    elif kind == "minecraft:looting_enchant":
        out = {"type": "minecraft:enchanted_count_increase", "enchantment": "minecraft:looting", **f}
    else:
        out = {"type": kind, **f}
    if conds:
        out["condition"] = condition(conds)
    return out


def node(v):
    """A pool or an entry (and an alternatives' children)."""
    v = dict(v)
    if "conditions" in v:
        v["condition"] = condition(v.pop("conditions"))
    if "functions" in v:
        fs = [x for x in (function(f) for f in v.pop("functions")) if x is not None]
        if fs:
            v["modifier"] = fs[0] if len(fs) == 1 else fs
    for k in ("entries", "children"):
        if k in v:
            v[k] = [node(e) for e in v[k]]
    return v


def convert(data):
    doc = json.loads(data.decode("utf-8-sig"))
    doc = node(doc)
    doc["pools"] = [node(p) for p in doc.get("pools", [])]
    return json.dumps(doc, indent=1).encode()


def write(files):
    n = 0
    for name in list(files):
        if "/loot_table/" in name and name.startswith("data/") and name.endswith(".json"):
            files[name] = convert(files[name])
            n += 1
    return n
