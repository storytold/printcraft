"""Remote REST SDK Client for PdfCraft (Issue #872).

# Architecture Reference
Module: sdks/python/pdfcraft/rest.py
Purpose:
    Connects to an on-premise PdfCraft REST daemon over HTTP/HTTPS,
    providing complete semantic parity with LocalClient.
"""

from __future__ import annotations

import base64
import json
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any, Dict, List, Optional, Union

from pdfcraft.errors import (
    ConflictError,
    InvalidArgumentError,
    NotFoundError,
    PdfCraftError,
    QuotaExceededError,
)
from pdfcraft.types import DocumentResult, DocumentSource, MergeOptions, RenderOptions, SplitResult


class RestClient:
    """Client for communicating with an on-premise PdfCraft REST service."""

    def __init__(self, base_url: str = "http://127.0.0.1:8080", auth_token: Optional[str] = None):
        """Initializes the RestClient targeting a base URL."""
        self.base_url = base_url.rstrip("/")
        self.auth_token = auth_token

    def _request(
        self,
        method: str,
        path: str,
        body: Optional[Dict[str, Any]] = None,
        expected_status: int = 200,
    ) -> bytes:
        """Sends an HTTP request and returns response bytes or raises typed PdfCraftError."""
        url = f"{self.base_url}{path}"
        data = json.dumps(body).encode("utf-8") if body is not None else None
        req = urllib.request.Request(url, data=data, method=method)

        req.add_header("Content-Type", "application/json")
        if self.auth_token:
            req.add_header("Authorization", f"Bearer {self.auth_token}")

        try:
            with urllib.request.urlopen(req) as resp:
                status = resp.getcode()
                response_data = resp.read()
                if status not in (expected_status, 200, 202):
                    self._handle_error_response(response_data, status)
                return response_data
        except urllib.error.HTTPError as e:
            err_body = e.read()
            self._handle_error_response(err_body, e.code)
            raise PdfCraftError(f"HTTP error {e.code}", status_code=e.code) from e
        except urllib.error.URLError as e:
            raise PdfCraftError(f"Connection failure to {url}: {e.reason}") from e

    def _handle_error_response(self, body: bytes, status: int) -> None:
        """Parses RFC 7807 Problem Details and raises specific typed exception."""
        try:
            prob = json.loads(body.decode("utf-8"))
            detail = prob.get("detail", f"HTTP {status}")
            code = prob.get("code", "ERROR")
        except Exception:
            detail = f"HTTP error {status}"
            code = "ERROR"

        if status == 400:
            raise InvalidArgumentError(detail)
        if status == 404:
            raise NotFoundError(detail)
        if status == 409:
            raise ConflictError(detail)
        if status in (413, 429):
            raise QuotaExceededError(detail)
        raise PdfCraftError(detail, status_code=status, error_code=code)

    def merge(
        self,
        sources: List[Union[str, Path, bytes, DocumentSource]],
        options: Optional[MergeOptions] = None,
        async_job: bool = False,
    ) -> DocumentResult:
        """Merges multiple PDF documents via the REST daemon."""
        if not sources:
            raise InvalidArgumentError("Cannot merge zero files")

        files_payload = []
        for s in sources:
            if isinstance(s, DocumentSource):
                if s.data is not None:
                    files_payload.append({"data_base64": base64.b64encode(s.data).decode("utf-8"), "name": s.name})
                elif s.path is not None:
                    files_payload.append({"path": str(s.path)})
            elif isinstance(s, bytes):
                files_payload.append({"data_base64": base64.b64encode(s).decode("utf-8")})
            else:
                files_payload.append({"path": str(s)})

        opts = options or MergeOptions()
        payload = {
            "files": files_payload,
            "options": {"output_filename": opts.output_filename, "pages": opts.pages},
            "async_job": async_job,
        }

        resp_bytes = self._request("POST", "/v1/merged-pdf", payload, expected_status=202 if async_job else 200)

        if async_job:
            job_info = json.loads(resp_bytes.decode("utf-8"))
            return self._poll_job(job_info["id"])

        return DocumentResult(data=resp_bytes, mime_type="application/pdf", size_bytes=len(resp_bytes))

    def split(
        self,
        source: Union[str, Path, bytes, DocumentSource],
        every_n_pages: int = 1,
    ) -> SplitResult:
        """Splits a PDF document via the REST daemon."""
        if isinstance(source, DocumentSource):
            file_payload = (
                {"data_base64": base64.b64encode(source.data).decode("utf-8")}
                if source.data is not None
                else {"path": str(source.path)}
            )
        elif isinstance(source, bytes):
            file_payload = {"data_base64": base64.b64encode(source).decode("utf-8")}
        else:
            file_payload = {"path": str(source)}

        payload = {
            "file": file_payload,
            "mode": {"EveryNPages": every_n_pages},
            "async_job": False,
        }

        resp_bytes = self._request("POST", "/v1/split-pdf", payload)
        data = json.loads(resp_bytes.decode("utf-8"))
        return SplitResult(files=[Path(p) for p in data.get("files", [])])

    def render_page(
        self,
        source: Union[str, Path, bytes, DocumentSource],
        page: int,
        options: Optional[RenderOptions] = None,
    ) -> DocumentResult:
        """Renders a PDF page to image via the REST daemon."""
        if page < 1:
            raise InvalidArgumentError("Page index must be >= 1 (1-based index)")

        opts = options or RenderOptions()

        if isinstance(source, DocumentSource):
            file_payload = (
                {"data_base64": base64.b64encode(source.data).decode("utf-8")}
                if source.data is not None
                else {"path": str(source.path)}
            )
        elif isinstance(source, bytes):
            file_payload = {"data_base64": base64.b64encode(source).decode("utf-8")}
        else:
            file_payload = {"path": str(source)}

        payload = {
            "file": file_payload,
            "page": page,
            "options": {"dpi": opts.dpi, "format": opts.image_format},
            "async_job": False,
        }

        resp_bytes = self._request("POST", "/v1/page-preview", payload)
        return DocumentResult(data=resp_bytes, mime_type="image/png", size_bytes=len(resp_bytes))

    def _poll_job(self, job_id: str, timeout: float = 60.0) -> DocumentResult:
        """Polls an asynchronous job until completion."""
        deadline = time.time() + timeout
        while time.time() < deadline:
            resp = self._request("GET", f"/v1/jobs/{job_id}")
            job = json.loads(resp.decode("utf-8"))
            status = job.get("status")

            if status == "succeeded":
                result_bytes = self._request("GET", f"/v1/jobs/{job_id}/result")
                return DocumentResult(data=result_bytes, mime_type="application/pdf", size_bytes=len(result_bytes))
            if status in ("failed", "canceled"):
                raise PdfCraftError(job.get("error", f"Job ended in status {status}"))

            time.sleep(0.05)

        raise PdfCraftError("Job polling timeout exceeded")
