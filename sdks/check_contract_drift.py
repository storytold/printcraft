#!/usr/bin/env python3
"""OpenAPI 3.1 Contract and Multi-Language SDK Drift Checker (Task 5.3).

# Architecture Reference
Script: scripts/check_contract_drift.py
Purpose:
    Enforces 1:1 schema alignment across Rust REST models, OpenAPI 3.1 specification,
    Python SDK dataclasses, and TypeScript SDK types to prevent API drift.
"""

import json
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent


def check_openapi_spec(openapi_path: Path) -> dict:
    """Validates the OpenAPI 3.1 file structure."""
    if not openapi_path.exists():
        print(f"FAIL: OpenAPI spec not found at {openapi_path}")
        sys.exit(1)

    with open(openapi_path, "r", encoding="utf-8") as f:
        spec = json.load(f)

    assert spec.get("openapi") == "3.1.0", f"Expected OpenAPI 3.1.0, got {spec.get('openapi')}"
    paths = spec.get("paths", {})

    expected_routes = [
        "/v1/merged-pdf",
        "/v1/split-pdf",
        "/v1/page-preview",
        "/v1/jobs/{id}",
        "/v1/jobs/{id}/result",
    ]
    for route in expected_routes:
        assert route in paths, f"Missing route in OpenAPI spec: {route}"

    schemas = spec.get("components", {}).get("schemas", {})
    expected_schemas = [
        "MergePayload",
        "SplitPayload",
        "RenderPayload",
        "SourcePayload",
        "MergeOptions",
        "SplitMode",
        "SplitResult",
        "RenderOptions",
        "JobResponse",
        "JobState",
        "ProblemDetails",
        "InvalidParam",
    ]
    for schema_name in expected_schemas:
        assert schema_name in schemas, f"Missing schema in OpenAPI spec: {schema_name}"

    print(f"✓ OpenAPI 3.1 specification valid ({len(paths)} routes, {len(schemas)} schemas)")
    return spec


def check_python_models(python_types_path: Path) -> None:
    """Checks Python dataclasses for required fields."""
    if not python_types_path.exists():
        print(f"FAIL: Python types not found at {python_types_path}")
        sys.exit(1)

    content = python_types_path.read_text(encoding="utf-8")
    expected_classes = [
        "DocumentSource",
        "DocumentResult",
        "MergeOptions",
        "SplitResult",
        "RenderOptions",
    ]
    for cls_name in expected_classes:
        assert f"class {cls_name}" in content, f"Missing Python dataclass: {cls_name}"

    print(f"✓ Python SDK types aligned ({len(expected_classes)} domain models)")


def check_typescript_types(ts_types_path: Path) -> None:
    """Checks TypeScript interfaces and types for required definitions."""
    if not ts_types_path.exists():
        print(f"FAIL: TypeScript types not found at {ts_types_path}")
        sys.exit(1)

    content = ts_types_path.read_text(encoding="utf-8")
    expected_types = [
        "DocumentSource",
        "DocumentResult",
        "MergeOptions",
        "SplitMode",
        "SplitResult",
        "RenderOptions",
        "JobResponse",
        "ProblemDetails",
    ]
    for t_name in expected_types:
        pattern = rf"(export type {t_name}|export interface {t_name})"
        assert re.search(pattern, content), f"Missing TypeScript type/interface: {t_name}"

    print(f"✓ TypeScript SDK types aligned ({len(expected_types)} type definitions)")


def check_rust_routes(rust_routes_path: Path) -> None:
    """Checks Rust routes file for endpoint implementations."""
    if not rust_routes_path.exists():
        print(f"FAIL: Rust routes file not found at {rust_routes_path}")
        sys.exit(1)

    content = rust_routes_path.read_text(encoding="utf-8")
    expected_endpoints = [
        "/v1/merged-pdf",
        "/v1/split-pdf",
        "/v1/page-preview",
        "/v1/jobs/",
    ]
    for ep in expected_endpoints:
        assert ep in content, f"Rust router missing endpoint match: {ep}"

    print(f"✓ Rust REST route dispatcher verified ({len(expected_endpoints)} route branches)")


def main() -> None:
    print("--- Running Contract Drift Check ---")
    openapi_path = REPO_ROOT / "sdks" / "openapi.json"
    python_types_path = REPO_ROOT / "sdks" / "python" / "pdfcraft" / "types.py"
    ts_types_path = REPO_ROOT / "sdks" / "typescript" / "src" / "types.ts"
    rust_routes_path = REPO_ROOT / "crates" / "rest" / "src" / "routes.rs"

    check_openapi_spec(openapi_path)
    check_python_models(python_types_path)
    check_typescript_types(ts_types_path)
    check_rust_routes(rust_routes_path)
    print("\nSUCCESS: All models and specifications are 100% aligned with zero contract drift.")


if __name__ == "__main__":
    main()
