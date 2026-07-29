#!/usr/bin/env python3
"""Generate lilook's build-time schema from a pinned lilaq checkout.

Two extraction paths, per the Phase 0 findings:
  1. plot constructors  -> plain `#let f(...)` with tidy doc-comments
  2. model elements     -> elembic `e.element.declare` with `e.field(...)`

Reuses lilaq's own tidy parser (docs-site repo) as the reference implementation
for path 1. Path 2 is a paren-balanced scan, because `e.field(...)` declarations
are frequently split across lines.

Usage: extract_schema.py <lilaq-src-dir> <tidy.py-dir> <out.json>
"""
import collections
import json
import os
import re
import sys

# ---------------------------------------------------------------- helpers


def balanced(s: str, i: int) -> int:
    """Index just past the `)` matching the `(` at position i."""
    depth = 0
    in_str = False
    prev = ""
    while i < len(s):
        c = s[i]
        if c == '"' and prev != "\\":
            in_str = not in_str
        if not in_str:
            if c == "(":
                depth += 1
            elif c == ")":
                depth -= 1
                if depth == 0:
                    return i + 1
        prev = c
        i += 1
    raise ValueError("unbalanced parentheses")


def split_top(s: str):
    """Split on commas at depth 0, respecting (), [], {} and strings."""
    out, cur, depth, in_str, prev = [], "", 0, False, ""
    for c in s:
        if c == '"' and prev != "\\":
            in_str = not in_str
        if not in_str:
            if c in "([{":
                depth += 1
            elif c in ")]}":
                depth -= 1
            elif c == "," and depth == 0:
                out.append(cur)
                cur = ""
                prev = c
                continue
        cur += c
        prev = c
    if cur.strip():
        out.append(cur)
    return out


# ---------------------------------------------------------------- widgets

# Maps a type atom to the control lilook should render. `None` means "no direct
# widget" -- the inspector falls back to a source snippet with an edit-in-place
# escape hatch.
WIDGET = {
    "int": "integer",
    "float": "number",
    "str": "text",
    "bool": "toggle",
    "color": "color",
    "length": "length",
    "ratio": "ratio",
    "relative": "relative",
    "stroke": "stroke",
    "gradient": "gradient",
    "tiling": "tiling",
    "alignment": "alignment",
    "angle": "angle",
    "array": "array",
    "dictionary": "dictionary",
    "content": "content",
    "function": None,
    "duration": "duration",
    "datetime": "datetime",
    "auto": "sentinel",
    "none": "sentinel",
    "number": "number",
    "paint": "paint",
    "text": "text",
}

SENTINELS = {"auto", "none"}

# Type atoms that collapse to a single control even though they are distinct
# Typst types. Without this ~45% of parameters fall through to "variant".
FAMILY = {
    "int": "number", "float": "number",
    "color": "paint", "gradient": "paint", "tiling": "paint",
    "length": "length", "relative": "length", "ratio": "length",
    "str": "text", "content": "text",
}


def family_of(t):
    return FAMILY.get(t, t)


# Hand-curated union signatures. Phase 0 measured 43 distinct `variant` unions
# covering 112 parameters; curating the dozen most common collapses roughly half
# of them. This table IS the curation layer -- it is deliberately separate from
# the generated schema so regeneration never clobbers it.
CURATED = [
    ({"length", "color", "stroke", "gradient", "tiling", "dictionary"}, "stroke"),
    ({"array", "function"}, "data"),
    ({"int", "float", "array"}, "number-or-array"),
    ({"ratio", "int", "float", "duration", "array"}, "number-or-array"),
    ({"float", "relative", "duration"}, "number-or-array"),
    ({"lq.mark", "str"}, "mark"),
    ({"lq.mark", "str", "color", "stroke", "length"}, "mark"),
    ({"float", "relative", "datetime"}, "coordinate"),
    ({"array", "dictionary"}, "structured"),
    ({"relative", "dictionary"}, "structured"),
    ({"lq.scale", "str", "function"}, "scale"),
    ({"str", "lq.scale"}, "scale"),
    ({"color", "gradient", "tiling", "ratio"}, "paint"),
    ({"color", "gradient", "tiling", "array"}, "paint"),
    ({"array", "gradient"}, "paint"),
]


def widget_for(types):
    """Decide the control for a parameter given its type union."""
    if not types:
        return {"widget": "opaque", "sentinels": [], "concrete": []}
    sentinels = [t for t in types if t in SENTINELS]
    concrete = [t for t in types if t not in SENTINELS]
    # a union of only string literals is a dropdown
    if concrete and all(re.fullmatch(r'"[^"]*"', t) for t in concrete):
        return {
            "widget": "enum",
            "sentinels": sentinels,
            "concrete": concrete,
            "choices": [t.strip('"') for t in concrete],
        }
    if len(concrete) == 1:
        return {
            "widget": WIDGET.get(concrete[0]) or "opaque",
            "sentinels": sentinels,
            "concrete": concrete,
        }
    if not concrete:
        return {"widget": "sentinel", "sentinels": sentinels, "concrete": []}
    cset = set(concrete)
    for sig, w in CURATED:
        if cset == sig:
            return {"widget": w, "sentinels": sentinels, "concrete": concrete,
                    "curated": True}
    fams = {family_of(t) for t in concrete}
    if len(fams) == 1:
        fam = fams.pop()
        w = WIDGET.get(fam, fam)
        return {"widget": w or "opaque", "sentinels": sentinels, "concrete": concrete}
    mapped = [WIDGET.get(t) for t in concrete]
    if len(set(mapped)) == 1 and mapped[0]:
        return {"widget": mapped[0], "sentinels": sentinels, "concrete": concrete}
    return {"widget": "variant", "sentinels": sentinels, "concrete": concrete}


# ---------------------------------------------------------------- extract


def public_surface(src_dir):
    entry = open(os.path.join(src_dir, "lilaq.typ")).read()
    public = {}
    for m in re.finditer(r'#import\s+"([^"]+)"\s*:\s*([^\n]+)', entry):
        path, names = m.group(1), m.group(2)
        if names.strip().startswith("("):
            continue
        if names.strip() == "*":
            # Glob import: everything the module defines becomes public.
            # `lq.linspace` reaches users this way and was missing until a
            # schema/index cross-check test caught it.
            mod = os.path.join(src_dir, path)
            if os.path.exists(mod):
                for m2 in re.finditer(r"^#let\s+([\w\-]+)", open(mod).read(), re.M):
                    public[m2.group(1)] = path
            continue
        for n in names.split(","):
            n = n.strip()
            if n and n != "*":
                public[n] = path
    return public


def extract_functions(src_dir, tidy):
    defs = {}
    for root, _, files in os.walk(src_dir):
        for f in files:
            if not f.endswith(".typ"):
                continue
            path = os.path.join(root, f)
            parser = tidy.TypDocParser()
            parser.parse(open(path).read())
            rel = os.path.relpath(path, src_dir)
            for d in parser.definitions:
                d["file"] = rel.replace(os.sep, "/")
                defs.setdefault(d["name"], d)
    return defs


def extract_elements(src_dir):
    elements = {}
    for root, _, files in os.walk(src_dir):
        for f in files:
            if not f.endswith(".typ"):
                continue
            path = os.path.join(root, f)
            s = open(path).read()
            rel = os.path.relpath(path, src_dir).replace(os.sep, "/")
            for m in re.finditer(r"#let\s+([\w\-]+)\s*=\s*e\.element\.declare\s*\(", s):
                name = m.group(1)
                body = s[m.end() : balanced(s, m.end() - 1)]
                fields = []
                for fm in re.finditer(r"e\.field\s*\(", body):
                    inner = body[fm.end() : balanced(body, fm.end() - 1) - 1]
                    parts = split_top(inner)
                    if not parts:
                        continue
                    nm = re.match(r'\s*"([^"]+)"', parts[0])
                    if not nm:
                        continue
                    field = {
                        "name": nm.group(1),
                        "type_expr": parts[1].strip() if len(parts) > 1 else "",
                        "default": None,
                        "required": False,
                        "internal": nm.group(1).startswith("_"),
                    }
                    for p in parts[2:]:
                        p = p.strip()
                        if p.startswith("default:"):
                            field["default"] = p[len("default:") :].strip()
                        elif p.startswith("required:"):
                            field["required"] = p[len("required:") :].strip() == "true"
                    fields.append(field)
                elements[name] = {"file": rel, "fields": fields}
    return elements


def types_from_expr(expr):
    """Best-effort atoms out of an elembic type expression."""
    if not expr:
        return []
    e = expr
    e = re.sub(r"e\.types\.(union|option|smart|wrap|array|literal)\s*\(", "(", e)
    e = re.sub(r"fold:\s*\w+", "", e)
    atoms = re.findall(r'"[^"]*"|[A-Za-z_][\w\.\-]*', e)
    drop = {"e", "types", "fold", "none_", "any"}
    out = []
    for a in atoms:
        if a in drop:
            continue
        if a not in out:
            out.append(a)
    if "option" in expr or "e.types.option" in expr:
        if "none" not in out:
            out.insert(0, "none")
    if "smart" in expr:
        if "auto" not in out:
            out.insert(0, "auto")
    return out


# ---------------------------------------------------------------- main


def main():
    src_dir, tidy_dir, out_path = sys.argv[1], sys.argv[2], sys.argv[3]
    sys.path.insert(0, tidy_dir)
    import tidy  # noqa: E402

    version = "unknown"
    toml = os.path.join(os.path.dirname(src_dir.rstrip("/")), "typst.toml")
    if os.path.exists(toml):
        m = re.search(r'version\s*=\s*"([^"]+)"', open(toml).read())
        if m:
            version = m.group(1)

    public = public_surface(src_dir)
    funcs = extract_functions(src_dir, tidy)
    elements = extract_elements(src_dir)

    schema = {
        "lilaq_version": version,
        "functions": {},
        "elements": {},
        "stats": {},
    }

    n_params = n_typed = 0
    widget_hist = collections.Counter()

    for name in sorted(public):
        d = funcs.get(name)
        if not d:
            continue
        params = []
        for p in d.get("params", []):
            types = p.get("types") or []
            w = widget_for(types)
            widget_hist[w["widget"]] += 1
            n_params += 1
            n_typed += 1 if types else 0
            params.append(
                {
                    "name": p["name"],
                    "kind": "named" if "default" in p else "positional",
                    "default": p.get("default"),
                    "types": types,
                    "doc": (p.get("description") or "").strip(),
                    **w,
                }
            )
        schema["functions"][name] = {
            "file": d["file"],
            "doc": (d.get("description") or "").strip(),
            "params": params,
        }

    for name, el in sorted(elements.items()):
        fields = []
        for f in el["fields"]:
            if f["internal"]:
                continue
            types = types_from_expr(f["type_expr"])
            w = widget_for(types)
            fields.append({**f, "types": types, **w})
        schema["elements"][name] = {"file": el["file"], "fields": fields}

    schema["stats"] = {
        "functions": len(schema["functions"]),
        "elements": len(schema["elements"]),
        "params": n_params,
        "typed": n_typed,
        "widgets": dict(widget_hist),
    }

    with open(out_path, "w") as fh:
        json.dump(schema, fh, indent=1, sort_keys=True)

    s = schema["stats"]
    print(f"lilaq {version}: {s['functions']} functions, {s['elements']} elements")
    print(f"  parameters {s['params']}, typed {s['typed']} ({100*s['typed']/s['params']:.1f}%)")
    print("  widget assignment:")
    for k, v in sorted(s["widgets"].items(), key=lambda kv: -kv[1]):
        print(f"    {k:<10} {v:>4}")
    print(f"  -> {out_path}")


if __name__ == "__main__":
    main()
