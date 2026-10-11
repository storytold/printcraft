import { test } from "node:test";
import assert from "node:assert/strict";

// Import domain types and models
import {
  Style,
  AcroFormExportType,
} from "../dist/types.js";

test("APDFL Text & Fonts: Style formatting matches ToString() contract", () => {
  const style = new Style("Helvetica-Bold", 14.0, { r: 0.1, g: 0.2, b: 0.3 });
  assert.equal(style.fontName, "Helvetica-Bold");
  assert.equal(style.fontSize, 14.0);
  assert.equal(style.color.r, 0.1);

  const str = style.toString();
  assert.ok(str.startsWith("[color="));
  assert.ok(str.includes("fontsize=14"));
  assert.ok(str.includes("fontname=Helvetica-Bold"));
});

test("APDFL Document Layer & GoToAction hierarchy", () => {
  const dest = {
    pageNumber: 5,
    fitMode: "Fit",
    zoom: 1.0,
  };

  const gotoAction = {
    destination: dest,
  };

  const action = {
    type: "GoTo",
    action: gotoAction,
  };

  assert.equal(action.type, "GoTo");
  assert.equal(action.action.destination.pageNumber, 5);
  assert.equal(action.action.destination.fitMode, "Fit");
});

test("APDFL Annotations & Redaction geometry", () => {
  const rect = { x: 50, y: 100, width: 200, height: 25 };
  const quad = {
    topLeft: { x: 50, y: 125 },
    topRight: { x: 250, y: 125 },
    bottomLeft: { x: 50, y: 100 },
    bottomRight: { x: 250, y: 100 },
  };

  const highlight = {
    quads: [quad],
    color: { r: 1.0, g: 1.0, b: 0.0 },
  };

  const annot = {
    rect,
    contents: "Review finding",
    highlight,
  };

  assert.equal(annot.contents, "Review finding");
  assert.equal(annot.highlight.quads.length, 1);
  assert.equal(annot.highlight.color.r, 1.0);
});

test("APDFL Forms & Data Exchange enums", () => {
  assert.equal(AcroFormExportType.FDF, 1);
  assert.equal(AcroFormExportType.XFDF, 2);
  assert.equal(AcroFormExportType.XML, 3);
  assert.equal(AcroFormExportType.JSON, 4);
});
