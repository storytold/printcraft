/**
 * Remote REST SDK Client for PdfCraft (Issue #872).
 *
 * # Architecture Reference
 * Module: sdks/typescript/src/rest.ts
 * Purpose:
 *   Fetch-based client communicating with an on-premise PdfCraft REST daemon,
 *   providing 1:1 semantic parity with PdfCraftLocalClient.
 */

declare const Buffer: any;
import {
  ConflictError,
  InvalidArgumentError,
  NotFoundError,
  PdfCraftError,
  QuotaExceededError,
} from "./errors.js";
import {
  DocumentResult,
  DocumentSource,
  JobResponse,
  MergeOptions,
  ProblemDetails,
  RenderOptions,
  SplitMode,
  SplitResult,
} from "./types.js";

export interface RestClientOptions {
  baseUrl?: string;
  authToken?: string;
  timeoutMs?: number;
}

export class PdfCraftRestClient {
  public readonly baseUrl: string;
  public readonly authToken?: string;
  public readonly timeoutMs: number;

  constructor(options?: RestClientOptions) {
    this.baseUrl = (options?.baseUrl ?? "http://127.0.0.1:8080").replace(/\/+$/, "");
    this.authToken = options?.authToken;
    this.timeoutMs = options?.timeoutMs ?? 60000;
  }

  private async request(
    method: string,
    path: string,
    body?: any,
    expectedStatus = 200
  ): Promise<{ status: number; headers: Headers; data: Uint8Array }> {
    const url = `${this.baseUrl}${path}`;
    const headers: Record<string, string> = {
      "Content-Type": "application/json",
    };

    if (this.authToken) {
      headers["Authorization"] = `Bearer ${this.authToken}`;
    }

    const response = await fetch(url, {
      method,
      headers,
      body: body ? JSON.stringify(body) : undefined,
    });

    const status = response.status;
    const arrayBuffer = await response.arrayBuffer();
    const data = new Uint8Array(arrayBuffer);

    if (status !== expectedStatus && status !== 200 && status !== 202) {
      this.handleErrorResponse(data, status);
    }

    return { status, headers: response.headers, data };
  }

  private handleErrorResponse(data: Uint8Array, status: number): never {
    let problem: ProblemDetails;
    try {
      problem = JSON.parse(new TextDecoder().decode(data));
    } catch {
      problem = {
        type: "about:blank",
        title: "HTTP Error",
        status,
        detail: `HTTP request failed with status ${status}`,
        code: "HTTP_ERROR",
      };
    }

    if (status === 400) {
      throw new InvalidArgumentError(problem.detail, problem);
    }
    if (status === 404) {
      throw new NotFoundError(problem.detail, problem);
    }
    if (status === 409) {
      throw new ConflictError(problem.detail, problem);
    }
    if (status === 413 || status === 429) {
      throw new QuotaExceededError(problem.detail, problem);
    }
    throw new PdfCraftError(problem.detail, status, problem.code, problem);
  }

  /**
   * Merges multiple PDF documents via the REST daemon.
   */
  async merge(
    sources: (DocumentSource | string | Uint8Array)[],
    options?: MergeOptions,
    asyncJob = false
  ): Promise<DocumentResult> {
    if (!sources || sources.length === 0) {
      throw new InvalidArgumentError("Cannot merge zero files");
    }

    const filesPayload = sources.map((s) => {
      if (typeof s === "string") {
        return { path: s };
      }
      if (s instanceof Uint8Array) {
        return { data_base64: Buffer.from(s).toString("base64") };
      }
      if ("data" in s) {
        return { data_base64: Buffer.from(s.data).toString("base64"), name: s.name };
      }
      return { path: s.path };
    });

    const payload = {
      files: filesPayload,
      options: {
        output_filename: options?.outputFilename,
        pages: options?.pages,
      },
      async_job: asyncJob,
    };

    const res = await this.request(
      "POST",
      "/v1/merged-pdf",
      payload,
      asyncJob ? 202 : 200
    );

    if (asyncJob) {
      const job: JobResponse = JSON.parse(new TextDecoder().decode(res.data));
      return this.pollJob(job.id);
    }

    const mime = res.headers.get("content-type") ?? "application/pdf";
    return {
      type: "memory",
      data: res.data,
      mimeType: mime,
    };
  }

  /**
   * Splits a PDF document via the REST daemon.
   */
  async split(
    source: DocumentSource | string | Uint8Array,
    mode?: SplitMode
  ): Promise<SplitResult> {
    let filePayload: any;
    if (typeof source === "string") {
      filePayload = { path: source };
    } else if (source instanceof Uint8Array) {
      filePayload = { data_base64: Buffer.from(source).toString("base64") };
    } else if ("data" in source) {
      filePayload = { data_base64: Buffer.from(source.data).toString("base64"), name: source.name };
    } else {
      filePayload = { path: source.path };
    }

    const payload = {
      file: filePayload,
      mode: mode ?? { EveryNPages: 1 },
      async_job: false,
    };

    const res = await this.request("POST", "/v1/split-pdf", payload);
    return JSON.parse(new TextDecoder().decode(res.data));
  }

  /**
   * Renders a PDF page to image via the REST daemon.
   */
  async renderPage(
    source: DocumentSource | string | Uint8Array,
    page: number,
    options?: RenderOptions
  ): Promise<DocumentResult> {
    if (page < 1) {
      throw new InvalidArgumentError("Page index must be >= 1 (1-based index)");
    }

    let filePayload: any;
    if (typeof source === "string") {
      filePayload = { path: source };
    } else if (source instanceof Uint8Array) {
      filePayload = { data_base64: Buffer.from(source).toString("base64") };
    } else if ("data" in source) {
      filePayload = { data_base64: Buffer.from(source.data).toString("base64"), name: source.name };
    } else {
      filePayload = { path: source.path };
    }

    const payload = {
      file: filePayload,
      page,
      options: {
        dpi: options?.dpi ?? 150.0,
        format: options?.imageFormat ?? "png",
      },
      async_job: false,
    };

    const res = await this.request("POST", "/v1/page-preview", payload);
    const mime = res.headers.get("content-type") ?? "image/png";
    return {
      type: "memory",
      data: res.data,
      mimeType: mime,
    };
  }

  private async pollJob(jobId: string): Promise<DocumentResult> {
    const deadline = Date.now() + this.timeoutMs;
    while (Date.now() < deadline) {
      const res = await this.request("GET", `/v1/jobs/${jobId}`);
      const job: JobResponse = JSON.parse(new TextDecoder().decode(res.data));

      if (job.status === "succeeded") {
        const resultRes = await this.request("GET", `/v1/jobs/${jobId}/result`);
        const mime = resultRes.headers.get("content-type") ?? "application/pdf";
        return {
          type: "memory",
          data: resultRes.data,
          mimeType: mime,
        };
      }

      if (job.status === "failed" || job.status === "canceled") {
        throw new PdfCraftError(job.error ?? `Job ended in status ${job.status}`);
      }

      await new Promise((r) => setTimeout(r, 50));
    }

    throw new PdfCraftError("Job polling timeout exceeded");
  }
}
