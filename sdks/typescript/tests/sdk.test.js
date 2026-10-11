import { test } from "node:test";
import assert from "node:assert/strict";

// Import modules
import { PdfCraftLocalClient } from "../dist/local.js";
import { PdfCraftRestClient } from "../dist/rest.js";
import { InvalidArgumentError, PdfCraftError } from "../dist/errors.js";

test("LocalClient merges documents in memory", async () => {
  const local = new PdfCraftLocalClient();
  const doc1 = new TextEncoder().encode("%PDF-1.4 sample 1");
  const doc2 = new TextEncoder().encode("%PDF-1.4 sample 2");

  const res = await local.merge([doc1, doc2]);
  assert.equal(res.type, "memory");
  assert.equal(res.mimeType, "application/pdf");

  const text = new TextDecoder().decode(res.data);
  assert.ok(text.startsWith("%PDF-1.7"));
  assert.ok(text.includes("%PDF-1.4 sample 1"));
  assert.ok(text.includes("%PDF-1.4 sample 2"));
});

test("LocalClient empty sources throws InvalidArgumentError", async () => {
  const local = new PdfCraftLocalClient();
  await assert.rejects(async () => {
    await local.merge([]);
  }, InvalidArgumentError);
});

test("LocalClient split operation", async () => {
  const local = new PdfCraftLocalClient();
  const res = await local.split("doc.pdf");
  assert.equal(res.files.length, 2);
  assert.equal(res.files[0], "split_part_1.pdf");
});

test("LocalClient renderPage validates page >= 1", async () => {
  const local = new PdfCraftLocalClient();
  await assert.rejects(async () => {
    await local.renderPage("doc.pdf", 0);
  }, InvalidArgumentError);

  const res = await local.renderPage("doc.pdf", 1, { dpi: 300 });
  assert.equal(res.type, "memory");
  assert.equal(res.mimeType, "image/png");
  const text = new TextDecoder().decode(res.data);
  assert.ok(text.includes("DPI_300"));
});

test("RestClient maps RFC 7807 error details", async () => {
  const rest = new PdfCraftRestClient({ baseUrl: "http://127.0.0.1:9999" });

  assert.throws(() => {
    rest.handleErrorResponse(
      new TextEncoder().encode(
        JSON.stringify({ status: 400, detail: "Bad param", code: "BAD_REQUEST" })
      ),
      400
    );
  }, InvalidArgumentError);

  assert.throws(() => {
    rest.handleErrorResponse(
      new TextEncoder().encode(
        JSON.stringify({ status: 413, detail: "Limit exceeded", code: "QUOTA_EXCEEDED" })
      ),
      413
    );
  }, (err) => {
    assert.equal(err.statusCode, 413);
    assert.equal(err.errorCode, "QUOTA_EXCEEDED");
    return true;
  });
});
