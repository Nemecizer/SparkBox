#!/usr/bin/env python3
"""Reproduce the bundled documentation from the reviewed, hash-verified cache.

Offline only. Does not run upstream files or turn their commands into actions.
Usage: python3 scripts/playbooks/import-library.py /path/to/review-cache
"""
import csv
import hashlib
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
REVISION = "30883150541743d580c9c347700c6ac99fc396d2"
IMAGES = {
    "cutile-kernels": ["nvcr.io/nvidia/cuda:13.2.0-devel-ubuntu22.04"],
    "multi-modal-inference": ["nvcr.io/nvidia/pytorch:26.03-py3"],
    "nemo-fine-tune": ["nvcr.io/nvidia/nemo-automodel:26.08"],
    "nemotron": ["vllm/vllm-openai:v0.20.0"],
    "nvfp4-quantization": ["nvcr.io/nvidia/vllm:25.12.post1-py3"],
    "open-webui": ["ghcr.io/open-webui/open-webui:ollama"],
    "portfolio-optimization": ["nvcr.io/nvidia/rapidsai/notebooks:25.10-cuda13-py3.13"],
    "pytorch-fine-tune": ["nvcr.io/nvidia/pytorch:25.11-py3"],
    "sglang": ["lmsysorg/sglang:latest-cu130"],
    "single-cell": ["nvcr.io/nvidia/rapidsai/notebooks:25.10-cuda13-py3.13"],
    "speculative-decoding": ["nvcr.io/nvidia/tensorrt-llm/release:1.3.0rc12"],
    "trt-llm": ["nvcr.io/nvidia/tensorrt-llm/release:1.3.0rc13"],
    "unsloth": ["nvcr.io/nvidia/pytorch:25.11-py3"],
}
ALIASES = {"comfy-ui": "comfyui", "register-to-brev": "brev", "station-brev": "brev", "station-ai-skills": "dgx-station-ai-skills", "reachy-photo-booth": "spark-reachy-photo-booth"}


def main(cache):
    source = cache / "source"
    head = json.loads((cache / "head.json").read_text())
    tree = json.loads((cache / "tree.json").read_text())
    assert head["sha"] == REVISION
    assert tree["sha"] in {REVISION, head["commit"]["tree"]["sha"]}
    assert not tree["truncated"]
    entries = {e["path"]: e for e in tree["tree"]}
    documents = {}
    for d in json.loads((cache / "documents.json").read_text()):
        b = (source / d["path"]).read_bytes()
        assert hashlib.sha256(b).hexdigest() == d["sha256"]
        assert hashlib.sha1(b"blob " + str(len(b)).encode() + b"\0" + b).hexdigest() == d["git_blob"]
        assert entries[d["path"]]["sha"] == d["git_blob"]
        documents[d["path"]] = {"path": d["path"], "gitBlob": d["git_blob"], "sha256": d["sha256"], "text": b.decode()}
    current = set(re.findall(r"nvidia/(playbook-[a-z0-9-]+)", documents["README.md"]["text"]))
    curated = {"playbook-" + r["id"]: r for r in csv.DictReader((ROOT / "scripts/playbooks/curation.tsv").open(), delimiter="\t")}
    assert current == curated.keys(), (current - curated.keys(), curated.keys() - current)
    guides = []
    cards = {c["id"]: c for c in json.loads((cache / "review-cards.json").read_text())}
    for path, d in documents.items():
        bits = path.split("/")
        if len(bits) != 3 or bits[0] != "nvidia" or bits[2] != "README.md":
            continue
        ident = bits[1]
        published = ident in current
        alias = ALIASES.get(ident, ident.removeprefix("station-"))
        parent = curated.get("playbook-" + alias)
        c = curated.get(ident, parent)
        title = next((l[2:].strip() for l in d["text"].splitlines() if l.startswith("# ")), ident)
        overview = cards.get(ident, {}).get("overview", {})
        table = overview.get("Supported hardware platforms", "")
        platforms = [name for name in ["DGX Spark", "DGX Station", "RTX Spark", "GeForce RTX", "RTX PRO", "Mac"] if f"**{name}" in table]
        if not platforms:
            platforms = ["DGX Station" if ident.startswith("station-") or ident == "playbook-sglang-inference" else "DGX Spark"]
        if ident == "playbook-multi-gpu": platforms = ["GeForce RTX", "RTX PRO"]
        example = "test" in ident
        if example: platforms = ["Publishing example"]
        spark = published and "DGX Spark" in platforms and not example
        category = c["category"] if c else "Reference"
        actions = []
        if spark:
            actions = {"Connections": ["fabric", "topology", "connect", "updates"], "Serving": ["models", "providers", "chat", "endpoints"], "Training": ["models", "files", "workloads"], "Coding and agents": ["models", "providers", "chat", "files"], "Data science": ["files", "workloads"], "Images, video and robotics": ["models", "files", "workloads"]}.get(category, [])
        images = IMAGES.get(ident.removeprefix("playbook-"), []) if spark else []
        for img in images: assert img in d["text"], (ident, img)
        if c and published:
            steps = c["steps"].split(" / ")
            purpose, success, attention = (c[k] for k in ["purpose", "success", "attention"])
        else:
            purpose = f"Read NVIDIA's {title} reference and compare it with the current guides."
            steps = ["Read the original guide's prerequisites and hardware limits.", "Compare its versions and commands with the current library before choosing a procedure.", "Keep a record of the selected source and follow its validation and cleanup sections."]
            success = "You have identified the applicable hardware, source version and validation steps."
            attention = "This document is not linked as a current guide in NVIDIA's root index. It has no SparkBox execution actions."
        if not published:
            attention = "Older or alternate reference; not linked in NVIDIA's current root index. Compare the original text with a current guide before use. " + attention
        prerequisites = overview.get("Prerequisites", "")
        # Some guides repeat this heading for distributed paths. Keep all of the source;
        # label the excerpt, never pretend it is a complete readiness specification.
        guide = {"id": ident, "title": title, "published": published, "example": example, "platforms": platforms, "spark": spark,
                 "category": category, "purpose": purpose, "steps": steps, "success": success, "attention": attention,
                 "actions": actions, "images": images, "readme": path, "treeSha": entries[f"nvidia/{ident}"]["sha"],
                 "documents": sorted(p for p in documents if p.startswith(f"nvidia/{ident}/")),
                 "sourceSteps": [s["title"] for s in cards.get(ident, {}).get("steps", [])],
                 "hardware": table, "prerequisites": prerequisites, "timeRisk": overview.get("Time & risk", "")}
        guides.append(guide)
    guides.sort(key=lambda g: (not g["published"], g["title"].lower(), g["id"]))
    assert len(guides) == 136 and len(documents) == 293
    out = ROOT / "resources/playbooks"
    out.mkdir(parents=True, exist_ok=True)
    catalog = {"schema": 1, "repository": "NVIDIA/dgx-spark-playbooks", "revision": REVISION, "reviewed": "2026-11-06", "guides": guides}
    (out / "catalog.json").write_text(json.dumps(catalog, ensure_ascii=False, indent=2) + "\n")
    (out / "documents.json").write_text(json.dumps(documents, ensure_ascii=False, separators=(",", ":")) + "\n")
    for name in ["LICENSE", "LICENSE-3rd-party"]:
        (out / name).write_bytes((source / name).read_bytes())
    rows = ["# NVIDIA library coverage", "", f"Reviewed source: `{REVISION}`. All 136 top-level guides and all 293 Markdown/license documents are included. Original command blocks are reference material, never executable action definitions. Supporting scripts, notebooks and images are linked at this revision, not bundled or executed.", "", "| Guide | Listing | Platform | Category | Native support |", "|---|---|---|---|---|"]
    for g in guides:
        support = "Read-only prerequisite check" if g["spark"] else "Reference only"
        if g["images"]: support += "; reviewed image preparation"
        if g["actions"]: support += "; guided links: " + ", ".join(g["actions"])
        rows.append(f"| `{g['id']}` | {'Current' if g['published'] else 'Alternate / older'} | {', '.join(g['platforms'])} | {g['category']} | {support} |")
    (ROOT / "docs/HOWTO-COVERAGE.md").write_text("\n".join(rows) + "\n")
    print(f"Bundled {len(guides)} guides, {len(current)} current, {len(documents)} documents; {sum(bool(g['images']) for g in guides)} image-preparation skills.")


if __name__ == "__main__":
    main(pathlib.Path(sys.argv[1]))
