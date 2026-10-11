#!/usr/bin/env node
/**
 * PdfCraft SDK Capability Tour (TypeScript): a 5-page generated showcase PDF.
 *
 * # Architecture Reference
 * Script: docs/sdk/examples/typescript/sdk_capability_tour.ts
 * Purpose:
 *   Builds a visually rich, *interactive* PDF using the typed interfaces of
 *   `@pdfcraft/sdk` (`Style`, `Rect`, `Color`, `Page`, `StyleTransition`,
 *   `DocTextFinderConfig`, `DocTextFinderMatch`, `Bookmark`, `Annotation`,
 *   `Field`, `DocumentResult`, and the `DocumentSession` contract).
 *
 *   The SDK ships the `DocumentSession` *interface* but no implementation yet
 *   (the Python SDK has one). This example therefore includes a small,
 *   dependency-free `PdfDocumentSession implements DocumentSession` that
 *   serialises straight to PDF 1.7. It is a reference implementation of the
 *   contract, not part of the published package.
 *
 *     Page 1  Cover: generated plasma hero, live stat tiles, clickable contents.
 *     Page 2  Typography: font families, StyleTransition runs, wrapped boxes, palette.
 *     Page 3  Data: vector bar chart, raster donut + Julia-set plate, table.
 *     Page 4  Review: findText() hits become real Highlight annotations; link
 *             annotations (URI + GoTo); live AcroForm text fields and checkboxes.
 *     Page 5  Landscape timeline (illustrative data).
 *   Plus a document outline (bookmarks) and Info metadata.
 *
 * Usage (Node >= 22.18 runs .ts directly; run from the repo root):
 *   node docs/sdk/examples/typescript/sdk_capability_tour.ts
 *   node docs/sdk/examples/typescript/sdk_capability_tour.ts \
 *        --output sample-docs/outputs/sdk_capability_tour_ts.pdf --render-previews
 *
 * Prerequisite: build the SDK once:  (cd sdks/typescript && npm install && npm run build)
 * Shortcut:     (cd sdks/typescript && npm run example:tour)
 * Notes: text is plain ASCII (built-in Type 1 fonts); previews need `pdftoppm`.
 */

import { execFile } from "node:child_process";
import { mkdirSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { deflateSync } from "node:zlib";

import { Style } from "../../../../sdks/typescript/dist/index.js";
import type {
  Bookmark,
  Color,
  DocTextFinderConfig,
  DocTextFinderMatch,
  DocumentResult,
  DocumentSession,
  Page,
  Quad,
  Rect,
  StyleTransition,
} from "../../../../sdks/typescript/dist/index.js";

const run = promisify(execFile);
const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../../..");

// =============================================================================
// Reference DocumentSession implementation
// =============================================================================

type Op =
  | { k: "rect"; rect: Rect; fill?: Color; stroke?: Color; lw: number }
  | { k: "text"; text: string; x: number; y: number; style: Style }
  | { k: "image"; id: string; w: number; h: number; rgb: Uint8Array; rect: Rect };

interface LinkSpec {
  rect: Rect;
  uri?: string;
  gotoPage?: number;
}
interface HighlightSpec {
  quads: Quad[];
  color: Color;
  contents: string;
}
interface FieldSpec {
  name: string;
  rect: Rect;
  value: string;
  checkbox: boolean;
  checked: boolean;
}
interface PageData {
  info: Page;
  ops: Op[];
  links: LinkSpec[];
  highlights: HighlightSpec[];
  fields: FieldSpec[];
}

const FONTS = ["Helvetica", "Helvetica-Bold", "Times-Roman", "Times-Bold", "Courier", "Courier-Bold"] as const;

export function measureTextWidth(text: string, style: Style): number {
  const bold = style.fontName.toLowerCase().includes("bold");
  const mono = style.fontName.toLowerCase().includes("courier");
  let w = 0;
  for (const ch of text) {
    if (mono) w += style.fontSize * 0.6;
    else if (ch === " ") w += style.fontSize * 0.28;
    else if ("il.,:;!'|[]/\\".includes(ch)) w += style.fontSize * (bold ? 0.3 : 0.25);
    else if (ch !== ch.toLowerCase() || "mwMW@#%&".includes(ch)) w += style.fontSize * (bold ? 0.75 : 0.68);
    else w += style.fontSize * (bold ? 0.56 : 0.5);
  }
  return w;
}

export function wrapTextToWidth(text: string, maxWidth: number, style: Style): string[] {
  const lines: string[] = [];
  for (const para of text.split("\n")) {
    if (!para.trim()) {
      lines.push("");
      continue;
    }
    let cur = "";
    for (const word of para.split(" ").filter(Boolean)) {
      const test = cur ? `${cur} ${word}` : word;
      if (measureTextWidth(test, style) <= maxWidth) cur = test;
      else {
        if (cur) lines.push(cur);
        cur = word;
      }
    }
    if (cur) lines.push(cur);
  }
  return lines;
}

const num = (n: number): string => (Number.isFinite(n) ? n.toFixed(3).replace(/\.?0+$/, "") || "0" : "0");
const pdfStr = (s: string): string => `(${s.replace(/[\\()]/g, (c) => `\\${c}`)})`;
const rgb = (c: Color): string => `${num(c.r)} ${num(c.g)} ${num(c.b)}`;

export class PdfDocumentSession implements DocumentSession {
  private data: PageData[] = [];
  bookmarks: Bookmark[] = [];
  title = "Untitled";
  author = "PdfCraft SDK";

  get numPages(): number {
    return this.data.length;
  }
  get pages(): Page[] {
    return this.data.map((d) => d.info);
  }

  addPage(width = 612, height = 792, rotationDegrees = 0): Page {
    if (width <= 0 || height <= 0) throw new RangeError(`Page dimensions must be positive: ${width}x${height}`);
    if (rotationDegrees % 90 !== 0) throw new RangeError(`Rotation must be a multiple of 90: ${rotationDegrees}`);
    const box: Rect = { x: 0, y: 0, width, height };
    const info: Page = { pageNumber: this.data.length + 1, mediaBox: box, cropBox: box, rotationDegrees: ((rotationDegrees % 360) + 360) % 360 };
    this.data.push({ info, ops: [], links: [], highlights: [], fields: [] });
    return info;
  }

  private pg(n: number): PageData {
    const d = this.data[n - 1];
    if (!d) throw new RangeError(`Page index ${n} out of bounds (1..${this.data.length})`);
    return d;
  }

  getPage(pageNumber: number): Page {
    return this.pg(pageNumber).info;
  }

  modifyPage(pageNumber: number, rotationDegrees?: number, mediaBox?: Rect): Page {
    const p = this.pg(pageNumber).info;
    if (rotationDegrees !== undefined) {
      if (rotationDegrees % 90 !== 0) throw new RangeError(`Rotation must be a multiple of 90: ${rotationDegrees}`);
      p.rotationDegrees = ((rotationDegrees % 360) + 360) % 360;
    }
    if (mediaBox) {
      if (mediaBox.width <= 0 || mediaBox.height <= 0) throw new RangeError("Media box must be positive");
      p.mediaBox = mediaBox;
      p.cropBox = mediaBox;
    }
    return p;
  }

  removePage(pageNumber: number): Page {
    const removed = this.pg(pageNumber).info;
    this.data.splice(pageNumber - 1, 1);
    this.data.forEach((d, i) => (d.info.pageNumber = i + 1));
    return removed;
  }

  // ---- content -------------------------------------------------------------
  addRect(pageNumber: number, rect: Rect, fill?: Color, stroke?: Color, lw = 1): void {
    this.pg(pageNumber).ops.push({ k: "rect", rect, fill, stroke, lw });
  }

  addText(pageNumber: number, text: string, x: number, y: number, style: Style = new Style("Helvetica", 12)): void {
    this.pg(pageNumber).ops.push({ k: "text", text, x, y, style });
  }

  addTextBox(pageNumber: number, text: string, rect: Rect, style: Style, padding = 4, lineSpacing = 1.25): string[] {
    const lines = wrapTextToWidth(text, Math.max(10, rect.width - 2 * padding), style);
    let y = rect.y + rect.height - padding - style.fontSize;
    const placed: string[] = [];
    for (const line of lines) {
      if (y < rect.y + padding) break;
      this.addText(pageNumber, line, rect.x + padding, y, style);
      placed.push(line);
      y -= style.fontSize * lineSpacing;
    }
    return placed;
  }

  addStyledLine(pageNumber: number, x: number, y: number, baseText: string, transitions: StyleTransition[]): void {
    const sorted = [...transitions].sort((a, b) => a.charIndex - b.charIndex);
    let cx = x;
    sorted.forEach((t, i) => {
      const end = i + 1 < sorted.length ? sorted[i + 1]!.charIndex : baseText.length;
      const seg = baseText.slice(t.charIndex, end);
      if (seg) {
        this.addText(pageNumber, seg, cx, y, t.style);
        cx += measureTextWidth(seg, t.style);
      }
    });
  }

  findText(query: string, config: DocTextFinderConfig = {}): DocTextFinderMatch[] {
    if (!query) return [];
    let pattern = config.regex ? query : query.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    if (config.wholeWordsOnly) pattern = `\\b${pattern}\\b`;
    const re = new RegExp(pattern, config.caseSensitive ? "g" : "gi");
    const out: DocTextFinderMatch[] = [];
    for (const d of this.data) {
      for (const op of d.ops) {
        if (op.k !== "text") continue;
        for (const m of op.text.matchAll(re)) {
          const x = op.x + measureTextWidth(op.text.slice(0, m.index ?? 0), op.style);
          const box: Rect = { x, y: op.y, width: measureTextWidth(m[0], op.style), height: op.style.fontSize };
          out.push({ pageNumber: d.info.pageNumber, matchedText: m[0], boundingBox: box, quads: [rectToQuad(box)] });
        }
      }
    }
    return out;
  }

  /** `imageSource` is a binary PPM (P6); see `makePpm`. Returns the image id. */
  addImage(pageNumber: number, imageSource: Uint8Array, rect: Rect, imageId?: string): string {
    const { width, height, rgb: pixels } = parsePpm(imageSource);
    const d = this.pg(pageNumber);
    const id = imageId ?? `Im${d.ops.filter((o) => o.k === "image").length + 1}`;
    d.ops.push({ k: "image", id, w: width, h: height, rgb: pixels, rect });
    return id;
  }

  removeImage(pageNumber: number, imageId: string): boolean {
    const d = this.pg(pageNumber);
    const i = d.ops.findIndex((o) => o.k === "image" && o.id === imageId);
    if (i < 0) return false;
    d.ops.splice(i, 1);
    return true;
  }

  // ---- interactive content -------------------------------------------------
  addLink(pageNumber: number, rect: Rect, target: { uri: string } | { gotoPage: number }): void {
    this.pg(pageNumber).links.push({ rect, ...target });
  }
  addHighlight(pageNumber: number, quads: Quad[], color: Color, contents = ""): void {
    this.pg(pageNumber).highlights.push({ quads, color, contents });
  }
  addField(pageNumber: number, name: string, rect: Rect, value = "", opts: { checkbox?: boolean; checked?: boolean } = {}): void {
    this.pg(pageNumber).fields.push({ name, rect, value, checkbox: !!opts.checkbox, checked: !!opts.checked });
  }

  // ---- output --------------------------------------------------------------
  toBytes(): Uint8Array {
    if (this.data.length === 0) throw new Error("Cannot save an empty document");
    const objs: (string | Buffer)[] = [];
    const alloc = (): number => objs.push("") /* 1-based id */;
    const set = (id: number, body: string | Buffer): void => void (objs[id - 1] = body);
    const add = (body: string | Buffer): number => {
      const id = alloc();
      set(id, body);
      return id;
    };
    const stream = (dict: string, data: Buffer): Buffer =>
      Buffer.concat([Buffer.from(`<< ${dict} /Length ${data.length} >>\nstream\n`, "latin1"), data, Buffer.from("\nendstream", "latin1")]);

    const catalogId = alloc();
    const pagesId = alloc();
    const infoId = alloc();
    const fontIds: Record<string, number> = {};
    for (const f of FONTS) fontIds[f] = add(`<< /Type /Font /Subtype /Type1 /BaseFont /${f} /Encoding /WinAnsiEncoding >>`);
    const pageIds = this.data.map(() => alloc());
    const fieldIds: number[] = [];

    this.data.forEach((d, idx) => {
      const content: string[] = [];
      const xobjects: string[] = [];
      for (const op of d.ops) {
        if (op.k === "rect") {
          const { x, y, width, height } = op.rect;
          const parts = [`${num(op.lw)} w`];
          if (op.fill) parts.push(`${rgb(op.fill)} rg`);
          if (op.stroke) parts.push(`${rgb(op.stroke)} RG`);
          parts.push(`${num(x)} ${num(y)} ${num(width)} ${num(height)} re`);
          parts.push(op.fill && op.stroke ? "B" : op.fill ? "f" : "S");
          content.push(parts.join(" "));
        } else if (op.k === "text") {
          const font = (FONTS as readonly string[]).includes(op.style.fontName) ? op.style.fontName : "Helvetica";
          const t = Buffer.from(op.text, "latin1").toString("latin1");
          content.push(`BT /${font.replace("-", "_")} ${num(op.style.fontSize)} Tf ${rgb(op.style.color)} rg ${num(op.x)} ${num(op.y)} Td ${pdfStr(t)} Tj ET`);
        } else {
          const imgId = add(stream(`/Type /XObject /Subtype /Image /Width ${op.w} /Height ${op.h} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode`, deflateSync(op.rgb)));
          const name = `Im_${op.id}`;
          xobjects.push(`/${name} ${imgId} 0 R`);
          content.push(`q ${num(op.rect.width)} 0 0 ${num(op.rect.height)} ${num(op.rect.x)} ${num(op.rect.y)} cm /${name} Do Q`);
        }
      }
      const contentId = add(stream("/Filter /FlateDecode", deflateSync(Buffer.from(content.join("\n"), "latin1"))));

      const annots: number[] = [];
      for (const l of d.links) {
        const { x, y, width, height } = l.rect;
        const action = l.uri
          ? `/A << /S /URI /URI ${pdfStr(l.uri)} >>`
          : `/Dest [${pageIds[(l.gotoPage ?? 1) - 1]} 0 R /XYZ 0 ${num(this.data[(l.gotoPage ?? 1) - 1]!.info.mediaBox.height)} 0]`;
        annots.push(add(`<< /Type /Annot /Subtype /Link /Rect [${num(x)} ${num(y)} ${num(x + width)} ${num(y + height)}] /Border [0 0 0] ${action} >>`));
      }
      for (const h of d.highlights) {
        const xs = h.quads.flatMap((q) => [q.topLeft.x, q.topRight.x, q.bottomLeft.x, q.bottomRight.x]);
        const ys = h.quads.flatMap((q) => [q.topLeft.y, q.topRight.y, q.bottomLeft.y, q.bottomRight.y]);
        const qp = h.quads.map((q) => [q.topLeft, q.topRight, q.bottomLeft, q.bottomRight].map((p) => `${num(p.x)} ${num(p.y)}`).join(" ")).join(" ");
        annots.push(
          add(`<< /Type /Annot /Subtype /Highlight /F 4 /Rect [${num(Math.min(...xs))} ${num(Math.min(...ys))} ${num(Math.max(...xs))} ${num(Math.max(...ys))}] /QuadPoints [${qp}] /C [${rgb(h.color)}] /CA 0.8 /Contents ${pdfStr(h.contents)} /T (PdfCraft) >>`),
        );
      }
      const fontResources = Object.entries(fontIds).map(([n, id]) => `/${n.replace("-", "_")} ${id} 0 R`);
      for (const f of d.fields) {
        const { x, y, width, height } = f.rect;
        const rectS = `[${num(x)} ${num(y)} ${num(x + width)} ${num(y + height)}]`;
        const mk = "/MK << /BC [0.2 0.5 0.8] /BG [0.96 0.98 1] >>";
        let id: number;
        if (f.checkbox) {
          const yes = add(stream(`/Type /XObject /Subtype /Form /BBox [0 0 ${num(width)} ${num(height)}]`, Buffer.from(`0.96 0.98 1 rg 0 0 ${num(width)} ${num(height)} re f 0.2 0.5 0.8 RG 1.5 w 1 1 ${num(width - 2)} ${num(height - 2)} re S 0.1 0.4 0.75 RG 2.2 w 4 ${num(height * 0.5)} m ${num(width * 0.42)} 4 l ${num(width - 4)} ${num(height - 4)} l S`, "latin1")));
          const off = add(stream(`/Type /XObject /Subtype /Form /BBox [0 0 ${num(width)} ${num(height)}]`, Buffer.from(`0.96 0.98 1 rg 0 0 ${num(width)} ${num(height)} re f 0.2 0.5 0.8 RG 1.5 w 1 1 ${num(width - 2)} ${num(height - 2)} re S`, "latin1")));
          id = add(`<< /Type /Annot /Subtype /Widget /FT /Btn /T ${pdfStr(f.name)} /Rect ${rectS} /F 4 /V /${f.checked ? "Yes" : "Off"} /AS /${f.checked ? "Yes" : "Off"} /AP << /N << /Yes ${yes} 0 R /Off ${off} 0 R >> >> ${mk} >>`);
        } else {
          id = add(`<< /Type /Annot /Subtype /Widget /FT /Tx /T ${pdfStr(f.name)} /V ${pdfStr(f.value)} /Rect ${rectS} /F 4 /DA (/Courier 10 Tf 0.1 0.1 0.15 rg) ${mk} >>`);
        }
        annots.push(id);
        fieldIds.push(id);
      }

      const mb = d.info.mediaBox;
      set(
        pageIds[idx]!,
        `<< /Type /Page /Parent ${pagesId} 0 R /MediaBox [${num(mb.x)} ${num(mb.y)} ${num(mb.x + mb.width)} ${num(mb.y + mb.height)}] /Rotate ${d.info.rotationDegrees ?? 0} /Contents ${contentId} 0 R /Resources << /Font << ${fontResources.join(" ")} >>${xobjects.length ? ` /XObject << ${xobjects.join(" ")} >>` : ""} >>${annots.length ? ` /Annots [${annots.map((a) => `${a} 0 R`).join(" ")}]` : ""} >>`,
      );
    });

    // Outline
    let outlineRef = "";
    if (this.bookmarks.length) {
      const rootId = alloc();
      const make = (items: Bookmark[], parent: number): number[] => {
        const ids = items.map(() => alloc());
        items.forEach((b, i) => {
          const kids = b.children?.length ? make(b.children, ids[i]!) : [];
          const dest = b.destination ? `/Dest [${pageIds[b.destination.pageNumber - 1]} 0 R /Fit]` : "";
          set(
            ids[i]!,
            `<< /Title ${pdfStr(b.title)} /Parent ${parent} 0 R ${dest}${i > 0 ? ` /Prev ${ids[i - 1]} 0 R` : ""}${i + 1 < ids.length ? ` /Next ${ids[i + 1]} 0 R` : ""}${kids.length ? ` /First ${kids[0]} 0 R /Last ${kids[kids.length - 1]} 0 R /Count ${kids.length}` : ""} >>`,
          );
        });
        return ids;
      };
      const top = make(this.bookmarks, rootId);
      set(rootId, `<< /Type /Outlines /First ${top[0]} 0 R /Last ${top[top.length - 1]} 0 R /Count ${top.length} >>`);
      outlineRef = `/Outlines ${rootId} 0 R /PageMode /UseOutlines`;
    }

    const helvId = fontIds["Helvetica"]!;
    const acro = fieldIds.length
      ? `/AcroForm << /Fields [${fieldIds.map((i) => `${i} 0 R`).join(" ")}] /NeedAppearances true /DA (/Helv 10 Tf 0 g) /DR << /Font << /Helv ${helvId} 0 R /Courier ${fontIds["Courier"]} 0 R >> >> >>`
      : "";
    set(catalogId, `<< /Type /Catalog /Pages ${pagesId} 0 R ${outlineRef} ${acro} >>`);
    set(pagesId, `<< /Type /Pages /Count ${pageIds.length} /Kids [${pageIds.map((i) => `${i} 0 R`).join(" ")}] >>`);
    set(infoId, `<< /Title ${pdfStr(this.title)} /Author ${pdfStr(this.author)} /Creator (PdfCraft TypeScript SDK example) >>`);

    const parts: Buffer[] = [Buffer.from("%PDF-1.7\n%\xE2\xE3\xCF\xD3\n", "latin1")];
    const offsets: number[] = [];
    let pos = parts[0]!.length;
    objs.forEach((body, i) => {
      const b = Buffer.concat([Buffer.from(`${i + 1} 0 obj\n`, "latin1"), Buffer.isBuffer(body) ? body : Buffer.from(body, "latin1"), Buffer.from("\nendobj\n", "latin1")]);
      offsets.push(pos);
      pos += b.length;
      parts.push(b);
    });
    const xref = [`xref\n0 ${objs.length + 1}\n0000000000 65535 f \n`, ...offsets.map((o) => `${String(o).padStart(10, "0")} 00000 n \n`)].join("");
    parts.push(Buffer.from(`${xref}trailer\n<< /Size ${objs.length + 1} /Root ${catalogId} 0 R /Info ${infoId} 0 R >>\nstartxref\n${pos}\n%%EOF\n`, "latin1"));
    return Buffer.concat(parts);
  }

  async save(targetPath: string): Promise<DocumentResult> {
    mkdirSync(path.dirname(targetPath), { recursive: true });
    writeFileSync(targetPath, this.toBytes());
    return { type: "file", path: targetPath, mimeType: "application/pdf", sizeBytes: statSync(targetPath).size };
  }

  async renderPage(pageNumber: number, outputPath: string, dpi = 150): Promise<string> {
    this.pg(pageNumber);
    mkdirSync(path.dirname(outputPath), { recursive: true });
    const tmp = outputPath.replace(/\.png$/, ".tmp.pdf");
    await this.save(tmp);
    // pdftoppm -singlefile writes <prefix>.png
    await run("pdftoppm", ["-png", "-singlefile", "-r", String(Math.round(dpi)), "-f", String(pageNumber), "-l", String(pageNumber), tmp, outputPath.replace(/\.png$/, "")]);
    (await import("node:fs")).unlinkSync(tmp);
    return outputPath;
  }

  stats() {
    const all = this.data.flatMap((d) => d.ops);
    return {
      pages: this.numPages,
      textRuns: all.filter((o) => o.k === "text").length,
      shapes: all.filter((o) => o.k === "rect").length,
      images: all.filter((o) => o.k === "image").length,
      annotations: this.data.reduce((n, d) => n + d.links.length + d.highlights.length + d.fields.length, 0),
    };
  }
}

function rectToQuad(r: Rect): Quad {
  return {
    topLeft: { x: r.x, y: r.y + r.height },
    topRight: { x: r.x + r.width, y: r.y + r.height },
    bottomLeft: { x: r.x, y: r.y },
    bottomRight: { x: r.x + r.width, y: r.y },
  };
}

// =============================================================================
// Raster helpers (PPM in, no external dependencies)
// =============================================================================

function makePpm(width: number, height: number, shader: (x: number, y: number) => [number, number, number]): Uint8Array {
  const header = Buffer.from(`P6\n${width} ${height}\n255\n`, "latin1");
  const body = Buffer.alloc(width * height * 3);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const [r, g, b] = shader(x, y);
      const o = (y * width + x) * 3;
      body[o] = clamp8(r);
      body[o + 1] = clamp8(g);
      body[o + 2] = clamp8(b);
    }
  }
  return Buffer.concat([header, body]);
}

function parsePpm(bytes: Uint8Array): { width: number; height: number; rgb: Uint8Array } {
  const buf = Buffer.from(bytes);
  const m = /^P6\s+(\d+)\s+(\d+)\s+255\s/.exec(buf.subarray(0, 40).toString("latin1"));
  if (!m) throw new TypeError("addImage expects a binary PPM (P6, maxval 255)");
  const width = Number(m[1]);
  const height = Number(m[2]);
  const rgbData = buf.subarray(m[0].length);
  if (rgbData.length < width * height * 3) throw new TypeError("PPM pixel data is truncated");
  return { width, height, rgb: rgbData.subarray(0, width * height * 3) };
}

const clamp8 = (v: number): number => Math.max(0, Math.min(255, Math.round(v)));

function hsv(h: number, s = 0.65, v = 0.92): Color {
  const [r, g, b] = hsvToRgb(h, s, v);
  return { r, g, b };
}
function hsvToRgb(h: number, s: number, v: number): [number, number, number] {
  h = ((h % 1) + 1) % 1;
  const i = Math.floor(h * 6);
  const f = h * 6 - i;
  const p = v * (1 - s);
  const q = v * (1 - f * s);
  const t = v * (1 - (1 - f) * s);
  return ([[v, t, p], [q, v, p], [p, v, t], [p, q, v], [t, p, v], [v, p, q]] as [number, number, number][])[i % 6]!;
}

/** Three-source plasma / interference field, mapped to a cyan -> indigo -> magenta ramp. */
function plasmaHero(width: number, height: number): Uint8Array {
  return makePpm(width, height, (x, y) => {
    const v = Math.sin(x * 0.021 + Math.sin(y * 0.03) * 2) + Math.sin(y * 0.027 + Math.sin(x * 0.02) * 2.5) + Math.sin(Math.hypot(x - width * 0.7, y - height * 0.3) * 0.05);
    const t = (v / 3 + 1) / 2;
    const [r, g, b] = hsvToRgb(0.5 + 0.38 * t, 0.78, 0.2 + 0.8 * t ** 1.4);
    return [r * 255, g * 255, b * 255];
  });
}

function juliaPlate(width: number, height: number, maxIter = 90): Uint8Array {
  const cr = -0.8;
  const ci = 0.156;
  return makePpm(width, height, (px, py) => {
    let zr = (px / width - 0.5) * 3.2;
    let zi = (py / height - 0.5) * 2.1;
    let n = 0;
    while (zr * zr + zi * zi <= 4 && n < maxIter) {
      [zr, zi] = [zr * zr - zi * zi + cr, 2 * zr * zi + ci];
      n++;
    }
    if (n === maxIter) return [8, 10, 24];
    const [r, g, b] = hsvToRgb(0.95 - 0.7 * Math.sqrt(n / maxIter), 0.75, 1);
    return [r * 255, g * 255, b * 255];
  });
}

function donut(size: number, shares: number[], colors: Color[], bg: Color): Uint8Array {
  const total = shares.reduce((a, b) => a + b, 0);
  const cuts = shares.map((_, i) => (shares.slice(0, i + 1).reduce((a, b) => a + b, 0) / total) * Math.PI * 2);
  const c = size / 2;
  return makePpm(size, size, (x, y) => {
    const dx = x + 0.5 - c;
    const dy = y + 0.5 - c;
    const rad = Math.hypot(dx, dy);
    const out = c - 2;
    if (rad > out + 1 || rad < out * 0.58) return [bg.r * 255, bg.g * 255, bg.b * 255];
    const edge = Math.min(1, Math.max(0, out + 1 - rad), Math.max(0, rad - out * 0.58 + 1)); // soft edge
    let a = Math.atan2(dx, -dy);
    if (a < 0) a += Math.PI * 2;
    const seg = cuts.findIndex((cut) => a <= cut + 1e-9);
    const col = colors[Math.max(0, seg) % colors.length]!;
    const gap = cuts.some((cut) => Math.abs(a - cut) < 0.018) ? 0 : 1;
    const k = gap * edge;
    return [(bg.r + (col.r - bg.r) * k) * 255, (bg.g + (col.g - bg.g) * k) * 255, (bg.b + (col.b - bg.b) * k) * 255];
  });
}

// =============================================================================
// Palette & layout helpers
// =============================================================================

const col = (r: number, g: number, b: number): Color => ({ r, g, b });
const INK = col(0.06, 0.08, 0.14);
const NIGHT = col(0.03, 0.05, 0.11);
const PAPER = col(0.975, 0.98, 0.985);
const CYAN = col(0.1, 0.72, 0.82);
const AMBER = col(0.98, 0.72, 0.15);
const ROSE = col(0.92, 0.28, 0.45);
const INDIGO = col(0.34, 0.33, 0.85);
const MUTED = col(0.42, 0.46, 0.54);
const RULE = col(0.84, 0.86, 0.9);
const WHITE = col(1, 1, 1);
const PW = 612;
const PH = 792;
const S = (font: string, size: number, color: Color = INK): Style => new Style(font, size, color);

function frame(doc: PdfDocumentSession, page: number, bg: Color, w = PW, h = PH): void {
  doc.addRect(page, { x: 0, y: 0, width: w, height: h }, bg);
}
function heading(doc: PdfDocumentSession, page: number, kicker: string, title: string): void {
  doc.addRect(page, { x: 48, y: 716, width: 36, height: 5 }, AMBER);
  doc.addText(page, kicker.toUpperCase(), 48, 730, S("Helvetica-Bold", 9, INDIGO));
  doc.addText(page, title, 48, 684, S("Helvetica-Bold", 30, INK));
}
function footer(doc: PdfDocumentSession, page: number, w = PW, dark = false): void {
  const c = dark ? col(0.6, 0.66, 0.78) : MUTED;
  doc.addRect(page, { x: 48, y: 46, width: w - 96, height: 0.8 }, c);
  doc.addText(page, "PdfCraft TypeScript SDK  |  generated by sdk_capability_tour.ts", 48, 30, S("Helvetica", 8, c));
  doc.addText(page, String(page).padStart(2, "0"), w - 64, 30, S("Helvetica-Bold", 9, c));
}

// =============================================================================
// Pages
// =============================================================================

const SECTIONS: [string, number][] = [
  ["Typography & Style", 2],
  ["Charts & Imagery", 3],
  ["Review, Links & Forms", 4],
  ["Timeline", 5],
];

function pageCover(doc: PdfDocumentSession, stats: ReturnType<PdfDocumentSession["stats"]>): void {
  const p = doc.addPage(PW, PH).pageNumber;
  frame(doc, p, NIGHT);
  doc.addRect(p, { x: 0, y: 0, width: 14, height: PH }, CYAN);
  doc.addText(p, "PDFCRAFT  /  TYPESCRIPT SDK", 48, 736, S("Helvetica-Bold", 10, AMBER));
  doc.addText(p, "Typed interfaces,", 48, 668, S("Helvetica-Bold", 42, WHITE));
  doc.addText(p, "real PDFs.", 48, 620, S("Helvetica-Bold", 42, hsv(0.5, 0.55, 1)));
  doc.addText(p, "Style, Rect, Annotation, Field and Bookmark, from the SDK's", 48, 586, S("Helvetica", 12.5, col(0.75, 0.8, 0.9)));
  doc.addText(p, "type definitions, compiled into links, highlights and form fields.", 48, 570, S("Helvetica", 12.5, col(0.75, 0.8, 0.9)));

  doc.addRect(p, { x: 46, y: 276, width: 520, height: 264 }, AMBER);
  doc.addImage(p, plasmaHero(516, 260), { x: 48, y: 278, width: 516, height: 260 }, "Hero");

  // Clickable contents (GoTo link annotations)
  doc.addText(p, "CONTENTS  (click)", 48, 244, S("Helvetica-Bold", 9, AMBER));
  SECTIONS.forEach(([name, target], i) => {
    const y = 218 - i * 22;
    doc.addText(p, String(target).padStart(2, "0"), 48, y, S("Courier-Bold", 11, CYAN));
    doc.addText(p, name, 80, y, S("Helvetica", 12, WHITE));
    doc.addRect(p, { x: 80 + measureTextWidth(name, S("Helvetica", 12)) + 8, y: y + 4, width: 120, height: 0.5 }, col(0.3, 0.36, 0.5));
    doc.addLink(p, { x: 44, y: y - 4, width: 300, height: 18 }, { gotoPage: target });
  });

  const tiles: [string, string, Color][] = [
    [String(stats.pages), "pages", CYAN],
    [String(stats.textRuns), "text runs", ROSE],
    [String(stats.shapes), "vector shapes", INDIGO],
    [String(stats.annotations), "annotations", AMBER],
  ];
  tiles.forEach(([v, label, c], i) => {
    const x = 340 + (i % 2) * 114;
    const y = 160 - Math.floor(i / 2) * 62;
    doc.addRect(p, { x, y, width: 106, height: 54 }, col(0.08, 0.11, 0.21), c, 1.3);
    doc.addRect(p, { x, y: y + 48, width: 106, height: 6 }, c);
    doc.addText(p, v, x + 10, y + 16, S("Helvetica-Bold", 20, WHITE));
    doc.addText(p, label, x + 10, y + 5, S("Helvetica", 8, col(0.7, 0.76, 0.88)));
  });
  doc.addText(p, "Tile values are measured from the live session.", 340, 76, S("Helvetica", 8, col(0.55, 0.6, 0.72)));
  footer(doc, p, PW, true);
}

function pageTypography(doc: PdfDocumentSession): void {
  const p = doc.addPage(PW, PH).pageNumber;
  frame(doc, p, PAPER);
  heading(doc, p, "02  /  Typography", "Type & Style");

  FONTS.forEach((font, i) => {
    const x = 48 + (i % 3) * 176;
    const y = 640 - Math.floor(i / 3) * 70;
    doc.addRect(p, { x, y: y - 44, width: 164, height: 60 }, WHITE, RULE, 0.8);
    doc.addText(p, "Aa Gg 42", x + 12, y - 14, S(font, 24));
    doc.addText(p, font, x + 12, y - 36, S("Helvetica", 8, MUTED));
  });

  doc.addText(p, "StyleTransition: one line, many voices", 48, 484, S("Helvetica-Bold", 12, INDIGO));
  const line = "Type  the  SDK.  Build  the  doc.  Ship.";
  const t = (charIndex: number, font: string, c: Color): StyleTransition => ({ charIndex, style: S(font, 22, c) });
  doc.addStyledLine(p, 48, 452, line, [t(0, "Helvetica-Bold", ROSE), t(6, "Times-Roman", INK), t(12, "Courier-Bold", INDIGO), t(18, "Times-Roman", INK), t(24, "Helvetica-Bold", CYAN), t(31, "Times-Roman", INK), t(36, "Helvetica-Bold", AMBER)]);

  const body =
    "Every shape in this document is an ordinary TypeScript object: a Rect is {x, y, width, height}, a Color is {r, g, b}, " +
    "and a Style is a class with a toString() contract. Layout code reads like data, compiles under strict mode, " +
    "and produces the same bytes every time it runs.";
  const colW = 252;
  [["Structural typing", CYAN], ["Deterministic output", ROSE]].forEach(([title, c], i) => {
    const x = 48 + i * (colW + 12);
    doc.addRect(p, { x, y: 262, width: colW, height: 170 }, WHITE, RULE, 0.8);
    doc.addRect(p, { x, y: 426, width: colW, height: 6 }, c as Color);
    doc.addText(p, title as string, x + 12, 404, S("Helvetica-Bold", 13, c as Color));
    doc.addTextBox(p, body, { x: x + 6, y: 266, width: colW - 12, height: 130 }, S("Times-Roman", 10.5), 6, 1.35);
  });

  doc.addText(p, "Generated palette: 24 hues x 3 tones", 48, 232, S("Helvetica-Bold", 12, INDIGO));
  const cell = 19.5;
  [[0.85, 0.95], [0.6, 0.98], [0.35, 1]].forEach(([s, v], r) => {
    for (let c = 0; c < 24; c++) doc.addRect(p, { x: 48 + c * (cell + 2), y: 208 - r * (cell + 2), width: cell, height: cell }, hsv(c / 24, s, v));
  });
  doc.addText(p, "hsv() is a 6-line helper returning the SDK's Color shape; the grid is 72 addRect() calls.", 48, 100, S("Helvetica", 9, MUTED));
  footer(doc, p);
}

function pageData(doc: PdfDocumentSession): void {
  const p = doc.addPage(PW, PH).pageNumber;
  frame(doc, p, PAPER);
  heading(doc, p, "03  /  Data & Imagery", "Charts & Imagery");

  // Bar chart
  const cx = 48, cy = 450, cw = 516, ch = 190;
  doc.addText(p, "Requests per second by release (synthetic series)", cx, cy + ch + 10, S("Helvetica-Bold", 11));
  doc.addRect(p, { x: cx, y: cy, width: cw, height: ch }, WHITE, RULE, 0.8);
  for (let g = 1; g <= 4; g++) doc.addRect(p, { x: cx + 10, y: cy + 14 + g * 36, width: cw - 20, height: 0.5 }, RULE);
  const series = Array.from({ length: 16 }, (_, i) => 60 + 34 * Math.cos(i / 2.1) + 3 * i);
  const peak = Math.max(...series);
  series.forEach((v, i) => {
    const h = (v / peak) * 150;
    const x = cx + 18 + i * 31;
    doc.addRect(p, { x, y: cy + 14, width: 22, height: h }, hsv(0.52 - 0.5 * (i / 15) + 0.02, 0.75, 0.92));
    doc.addText(p, v.toFixed(0), x + 3, cy + 18 + h, S("Helvetica", 7, MUTED));
  });

  // Donut + legend
  const shares = [38, 27, 21, 14];
  const names = ["Render", "Parse", "Encode", "I/O"];
  const palette = [CYAN, INDIGO, ROSE, AMBER];
  doc.addRect(p, { x: 48, y: 186, width: 150, height: 214 }, WHITE, RULE, 0.8);
  doc.addImage(p, donut(120, shares, palette, WHITE), { x: 63, y: 270, width: 120, height: 120 }, "Donut");
  names.forEach((n, i) => {
    doc.addRect(p, { x: 62, y: 250 - i * 16, width: 9, height: 9 }, palette[i]);
    doc.addText(p, `${n}  ${shares[i]}%`, 78, 251 - i * 16, S("Helvetica", 9, INK));
  });

  // Julia plate
  doc.addRect(p, { x: 212, y: 200, width: 204, height: 200 }, INK);
  doc.addImage(p, juliaPlate(300, 196), { x: 214, y: 202, width: 200, height: 196 }, "Julia");
  doc.addText(p, "Plate I: Julia set, c = -0.8 + 0.156i", 212, 184, S("Helvetica-Bold", 9.5));
  doc.addText(p, "computed in TypeScript, embedded via addImage()", 212, 171, S("Helvetica", 8.5, MUTED));

  // Table
  const rows: [string, string][] = [["Method", "Returns"], ["addPage", "Page"], ["addText", "void"], ["findText", "Match[]"], ["addImage", "string"], ["save", "Promise"]];
  const tx = 430, rh = 33;
  rows.forEach(([a, b], r) => {
    const y = 200 + (rows.length - 1 - r) * rh;
    const head = r === 0;
    doc.addRect(p, { x: tx, y, width: 134, height: rh }, head ? INK : r % 2 ? WHITE : col(0.93, 0.95, 0.98), head ? undefined : RULE, 0.5);
    doc.addText(p, a, tx + 8, y + 12, S(head ? "Helvetica-Bold" : "Courier", 9, head ? WHITE : INK));
    doc.addText(p, b, tx + 76, y + 12, S("Helvetica-Bold", 9, head ? WHITE : INDIGO));
  });
  footer(doc, p);
}

function pageReview(doc: PdfDocumentSession): { errors: number; warns: number } {
  const p = doc.addPage(PW, PH).pageNumber;
  frame(doc, p, PAPER);
  heading(doc, p, "04  /  Review, Links & Forms", "Find, Mark, Link");

  const log = [
    "INFO  engine boot, 0 plugins",
    "WARN  font fallback for U+2603",
    "INFO  page 3 rendered in 11 ms",
    "ERROR xref entry 42 repaired",
    "INFO  incremental save complete",
    "WARN  image downsampled to 150 dpi",
    "ERROR signature digest mismatch",
  ];
  const panel: Rect = { x: 48, y: 468, width: 330, height: 190 };
  doc.addRect(p, panel, col(0.07, 0.09, 0.15));
  const mono = S("Courier", 10, col(0.8, 0.86, 0.95));
  log.forEach((txt, i) => doc.addText(p, txt, panel.x + 12, panel.y + panel.height - 24 - i * 22, mono));

  // findText -> visual swatch (under text) + a *real* Highlight annotation
  let errors = 0, warns = 0;
  for (const [word, c] of [["ERROR", ROSE], ["WARN", AMBER]] as [string, Color][]) {
    for (const m of doc.findText(`^${word}`, { regex: true, caseSensitive: true })) {
      if (m.pageNumber !== p) continue;
      word === "ERROR" ? errors++ : warns++;
      const b = m.boundingBox;
      doc.addHighlight(p, [rectToQuad({ x: b.x - 1, y: b.y - 2, width: b.width + 2, height: b.height + 2 })], c, `${word} hit found by findText()`);
    }
  }

  doc.addText(p, "findText() results", 400, 636, S("Helvetica-Bold", 12, INDIGO));
  [["ERROR", errors, ROSE], ["WARN", warns, AMBER]].forEach(([label, n, c], i) => {
    const y = 570 - i * 62;
    doc.addRect(p, { x: 400, y, width: 164, height: 52 }, WHITE, c as Color, 1.5);
    doc.addText(p, String(n), 414, y + 14, S("Helvetica-Bold", 26, c as Color));
    doc.addText(p, `${label} annotations`, 450, y + 20, S("Helvetica", 9.5));
  });
  doc.addText(p, "Open in a PDF viewer: the highlights are real annotations.", 48, 446, S("Helvetica", 9, MUTED));

  // Link annotations
  const links: [string, { uri: string } | { gotoPage: number }, Color][] = [
    ["Project README  (URI action)", { uri: "https://github.com/richfrem/pdfcraft" }, INDIGO],
    ["Back to cover  (GoTo action)", { gotoPage: 1 }, CYAN],
    ["Jump to timeline  (GoTo action)", { gotoPage: 5 }, ROSE],
  ];
  links.forEach(([label, target, c], i) => {
    const x = 48 + i * 176;
    doc.addRect(p, { x, y: 380, width: 164, height: 38 }, WHITE, c, 1.5);
    doc.addText(p, label.split("  ")[0]!, x + 10, 402, S("Helvetica-Bold", 10, c));
    doc.addText(p, label.split("  ")[1]!, x + 10, 388, S("Helvetica", 8, MUTED));
    doc.addLink(p, { x, y: 380, width: 164, height: 38 }, target);
  });

  // Form (real AcroForm widgets)
  doc.addRect(p, { x: 48, y: 120, width: 516, height: 230 }, WHITE, RULE, 0.8);
  doc.addRect(p, { x: 48, y: 344, width: 516, height: 6 }, INDIGO);
  doc.addText(p, "Interactive AcroForm: click and type", 62, 322, S("Helvetica-Bold", 12, INDIGO));
  const label = (t: string, x: number, y: number): void => doc.addText(p, t, x, y, S("Helvetica-Bold", 9.5, col(0.2, 0.25, 0.3)));
  label("Reviewer", 62, 296);
  doc.addField(p, "reviewer", { x: 62, y: 266, width: 230, height: 26 }, "Rin Chen");
  label("Date", 308, 296);
  doc.addField(p, "date", { x: 308, y: 266, width: 120, height: 26 }, "2026-10-10");
  label("Notes", 62, 244);
  doc.addField(p, "notes", { x: 62, y: 214, width: 366, height: 26 }, "Ship after signature check is fixed.");
  label("Release approved", 460, 296);
  doc.addField(p, "approved", { x: 460, y: 266, width: 22, height: 22 }, "", { checkbox: true, checked: true });
  label("Needs legal review", 460, 244);
  doc.addField(p, "legal", { x: 460, y: 214, width: 22, height: 22 }, "", { checkbox: true });
  doc.addText(p, "Fields are /Widget annotations registered in /AcroForm; the check boxes carry on/off appearance streams.", 62, 150, S("Helvetica", 8.5, MUTED));
  footer(doc, p);
  return { errors, warns };
}

function pageTimeline(doc: PdfDocumentSession): void {
  const w = PH, h = PW;
  const p = doc.addPage(w, h).pageNumber;
  frame(doc, p, NIGHT, w, h);
  doc.addRect(p, { x: 48, y: h - 76, width: 36, height: 5 }, AMBER);
  doc.addText(p, "05  /  PAGE GEOMETRY", 48, h - 62, S("Helvetica-Bold", 9, AMBER));
  doc.addText(p, "Landscape Timeline", 48, h - 106, S("Helvetica-Bold", 30, WHITE));
  const gx = 220, qw = 130;
  ["Q1", "Q2", "Q3", "Q4"].forEach((q, i) => {
    doc.addRect(p, { x: gx + i * qw, y: 150, width: qw - 2, height: 300 }, col(0.08, 0.11, 0.2));
    doc.addText(p, q, gx + i * qw + 10, 428, S("Helvetica-Bold", 12, col(0.7, 0.76, 0.9)));
  });
  const tracks: [string, number, number, Color][] = [
    ["Workstream A", 0, 2.2, CYAN],
    ["Workstream B", 0.8, 3, ROSE],
    ["Workstream C", 1.6, 3.6, INDIGO],
    ["Workstream D", 0.3, 1.6, AMBER],
    ["Workstream E", 2.2, 4, hsv(0.58, 0.6, 0.95)],
  ];
  tracks.forEach(([name, a, b, c], i) => {
    const y = 380 - i * 54;
    doc.addText(p, name, 48, y + 10, S("Helvetica", 11, col(0.85, 0.89, 0.96)));
    doc.addRect(p, { x: gx + a * qw, y, width: (b - a) * qw - 2, height: 30 }, c);
    doc.addText(p, `${(b - a).toFixed(1)} qtrs`, gx + a * qw + 10, y + 10, S("Helvetica-Bold", 10, NIGHT));
  });
  doc.addText(p, "Illustrative data. Page 5 was created with addPage(792, 612); modifyPage() can rotate it.", 48, 100, S("Helvetica", 9, col(0.6, 0.66, 0.78)));
  footer(doc, p, w, true);
}

// =============================================================================
// Driver
// =============================================================================

function populate(doc: PdfDocumentSession, stats: ReturnType<PdfDocumentSession["stats"]>): void {
  doc.title = "PdfCraft TypeScript SDK Capability Tour";
  pageCover(doc, stats);
  pageTypography(doc);
  pageData(doc);
  pageReview(doc);
  pageTimeline(doc);
  doc.bookmarks = [
    { title: "Cover", destination: { pageNumber: 1, fitMode: "Fit" } },
    ...SECTIONS.map(([title, pageNumber]): Bookmark => ({ title, destination: { pageNumber, fitMode: "Fit" } })),
  ];
}

async function main(): Promise<void> {
  const args = process.argv.slice(2);
  const flag = (n: string): string | undefined => (args.includes(n) ? args[args.indexOf(n) + 1] : undefined);
  const output = path.resolve(flag("--output") ?? path.join(REPO_ROOT, "sample-docs/outputs/sdk_capability_tour_ts.pdf"));

  // Dry run so the cover can report real numbers.
  const dry = new PdfDocumentSession();
  populate(dry, { pages: 5, textRuns: 0, shapes: 0, images: 0, annotations: 0 });
  const doc = new PdfDocumentSession();
  populate(doc, dry.stats());

  const result = await doc.save(output);
  if (result.type === "file") console.log(`Saved ${result.path} (${(result.sizeBytes / 1024).toFixed(1)} KiB, ${doc.numPages} pages)`);
  if (args.includes("--render-previews")) {
    for (let n = 1; n <= doc.numPages; n++) {
      const png = await doc.renderPage(n, path.join(path.dirname(output), "previews", `${path.basename(output, ".pdf")}_page_${n}.png`), 110);
      console.log(`  preview: ${png}`);
    }
  }
}

await main();
