#!/usr/bin/env python3
"""Checks of the packaging files: the doap file, the Flatpak manifest, the CI
workflow, the dependency minimums and leftovers of removed code.

usage: check-packaging.py SOURCE_ROOT

Prints "ok <check>" or "FAIL <check>: <reason>" per check; fails if any
check fails.
"""

import json
import pathlib
import re
import sys
import xml.etree.ElementTree as ET

ROOT = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else ".").resolve()
MANIFEST = ROOT / "build-aux/app.drey.PaperPlane.Devel.json"
DOAP_NS = "{http://usefulinc.com/ns/doap#}"

failures = []


def check(name):
    def wrap(fn):
        try:
            problem = fn()
        except Exception as e:  # noqa: BLE001
            problem = f"{type(e).__name__}: {e}"
        if problem:
            failures.append(name)
            print(f"FAIL {name}: {problem}")
        else:
            print(f"ok {name}")
        return fn

    return wrap


def manifest():
    return json.loads(MANIFEST.read_text())


def modules(data):
    for module in data.get("modules", []):
        if isinstance(module, dict):
            yield module
            yield from modules(module)


@check("doap is well formed")
def _():
    root = ET.parse(ROOT / "PaperPlane.doap").getroot()
    for field in ["name", "shortdesc", "homepage", "maintainer"]:
        if root.find(f"{DOAP_NS}{field}") is None:
            return f"no <{field}>"
    # Every maintainer is a closed element with a person and a name (#572)
    for maintainer in root.findall(f"{DOAP_NS}maintainer"):
        if maintainer.find(".//{http://xmlns.com/foaf/0.1/}name") is None:
            return "a <maintainer> without a foaf:name"


@check("flatpak runtime is GNOME 50")
def _():
    data = manifest()
    if data["runtime"] != "org.gnome.Platform" or data["runtime-version"] != "50":
        return f"{data['runtime']} {data['runtime-version']}"
    if data["sdk"] != "org.gnome.Sdk":
        return data["sdk"]


@check("flatpak uses the old GL renderer")
def _():
    if "--env=GSK_RENDERER=gl" not in manifest().get("finish-args", []):
        return "GSK_RENDERER=gl is not in finish-args"


@check("flatpak uses the GTK of the runtime")
def _():
    names = [m.get("name") for m in modules(manifest())]
    if "gtk" in names:
        return "the manifest builds its own GTK"
    patches = [
        s.get("path")
        for m in modules(manifest())
        for s in m.get("sources", [])
        if isinstance(s, dict) and s.get("type") == "patch"
    ]
    if patches:
        return f"patches: {patches}"
    if (ROOT / "build-aux/gtk-reversed-list.patch").exists():
        return "build-aux/gtk-reversed-list.patch is still there"


@check("flatpak builds TDLib from source, found with pkg-config")
def _():
    names = [m.get("name") for m in modules(manifest())]
    if "tdlib" not in names:
        return f"no tdlib module in {names}"
    app = manifest()["modules"][-1]
    if any("tdlib-backend" in arg for arg in app.get("config-opts", [])):
        return "the removed tdlib-backend option is still passed"


@check("CI builds with the image of the Flatpak runtime")
def _():
    workflow = (ROOT / ".github/workflows/ci.yml").read_text()
    images = re.findall(r"image:\s*\S*flatpak-github-actions:gnome-(\S+)", workflow)
    wanted = manifest()["runtime-version"]
    if not images:
        return "no flatpak-github-actions image"
    if any(image != wanted for image in images):
        return f"images gnome-{images}, runtime {wanted}"


def version_tuple(text):
    return tuple(int(part) for part in text.split("."))


@check("meson requires the library versions the Rust bindings use")
def _():
    cargo = (ROOT / "Cargo.toml").read_text()
    meson = (ROOT / "meson.build").read_text()

    def features(package):
        line = re.search(rf'^\w+ = \{{[^}}]*package = "{package}"[^}}]*\}}', cargo, re.M)
        return re.findall(r'"(v\d+_\d+|gnome_\d+)"', line.group(0)) if line else []

    def minimum(library):
        m = re.search(rf"dependency\('{re.escape(library)}', version: '>= ([\d.]+)'\)", meson)
        return version_tuple(m.group(1)) if m else None

    # What the features of gtk4-rs and libadwaita-rs need (their Cargo.toml).
    gnome = {"gnome_47": {"gtk4": "4.16", "gio-2.0": "2.82", "glib-2.0": "2.82"},
             "gnome_46": {"gtk4": "4.14", "gio-2.0": "2.80", "glib-2.0": "2.80"},
             "gnome_45": {"gtk4": "4.12", "gio-2.0": "2.78", "glib-2.0": "2.78"}}
    needed = {}
    for feature in features("gtk4"):
        if feature in gnome:
            needed.update(gnome[feature])
        elif feature.startswith("v"):
            needed["gtk4"] = feature[1:].replace("_", ".")
    for feature in features("libadwaita"):
        needed["libadwaita-1"] = feature[1:].replace("_", ".")

    problems = []
    for library, version in needed.items():
        have = minimum(library)
        if have is None or have < version_tuple(version):
            problems.append(f"{library} >= {version} needed, meson asks for {have}")
    return "; ".join(problems)


@check("no clone! of the old syntax")
def _():
    old = [
        f"{path.relative_to(ROOT)}"
        for path in (ROOT / "src").rglob("*.rs")
        if re.search(r"clone!\(\s*@(weak|strong)", path.read_text())
    ]
    if old:
        return ", ".join(old)


@check("the removed autoplay setting left nothing behind")
def _():
    schema = (ROOT / "data/app.drey.PaperPlane.gschema.xml.in").read_text()
    if "autoplay" in schema:
        return "the schema still has an autoplay key"
    leftovers = [
        f"{path.relative_to(ROOT)}"
        for path in (ROOT / "src").rglob("*")
        if path.suffix in (".rs", ".blp") and "autoplay" in path.read_text()
    ]
    if leftovers:
        return ", ".join(leftovers)


sys.exit(1 if failures else 0)
