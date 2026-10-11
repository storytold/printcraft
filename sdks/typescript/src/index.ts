/**
 * Public exports for @pdfcraft/sdk (Issue #872).
 *
 * # Architecture Reference
 * Module: sdks/typescript/src/index.ts
 * Purpose:
 *   Root exports providing unified in-process and REST document processing clients.
 */

export * from "./types.js";
export * from "./errors.js";
export * from "./local.js";
export * from "./rest.js";
