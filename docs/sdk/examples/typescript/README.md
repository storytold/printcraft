# TypeScript SDK examples

## Prerequisites

| Need | How |
|---|---|
| **Node 22.18+** (runs `.ts` directly via type stripping; older Node fails with `Unknown file extension ".ts"`) | `node --version`; get it with `brew install node` or <https://nodejs.org> |
| npm | ships with Node |
| `pdftoppm` (only for `--render-previews`) | `brew install poppler` |

## One-time setup and run

The example imports the SDK from its **built** `sdks/typescript/dist/`, so build it first.
The npm script does everything:

```bash
cd sdks/typescript
npm install
npm run example:tour          # builds the SDK, writes the PDF and PNG previews
open ../../sample-docs/outputs/sdk_capability_tour_ts.pdf    # macOS
```

Or run the pieces yourself, from the repository root:

```bash
(cd sdks/typescript && npm install && npm run build)
node docs/sdk/examples/typescript/sdk_capability_tour.ts
node docs/sdk/examples/typescript/sdk_capability_tour.ts --output sample-docs/outputs/my.pdf --render-previews
```

Type-check the example (strict):

```bash
cd sdks/typescript && npm run typecheck:examples
```

## What it shows

`sdk_capability_tour.ts` builds a 5-page PDF from the SDK's TypeScript types. The SDK ships the
`DocumentSession` *interface* but no implementation, so the example includes a small reference
`PdfDocumentSession` that serializes to PDF. It produces real link and highlight annotations,
AcroForm fields and a bookmark outline, so open the PDF in Preview or Acrobat to try them.

## Troubleshooting

- `Cannot find module .../dist/index.js`: run `npm run build` in `sdks/typescript`.
- `Unknown file extension ".ts"`: Node is older than 22.18.
- `pdftoppm: command not found` with `--render-previews`: `brew install poppler`.
- Editor shows `Cannot find name 'Buffer'`: run `npm install` in `sdks/typescript`
  (installs `@types/node`).
