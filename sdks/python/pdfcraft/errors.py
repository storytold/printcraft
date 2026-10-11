"""Error hierarchies and RFC 7807 Problem Details for PdfCraft Python SDK.

# Architecture Reference
Module: sdks/python/pdfcraft/errors.py
Purpose:
    Provides structured, actionable exceptions mapped cleanly from engine errors
    and HTTP Problem Details.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Dict, List, Optional


@dataclass
class ProblemDetails:
    """RFC 7807 Problem Details representation."""

    type: str
    title: str
    status: int
    detail: str
    code: str
    instance: Optional[str] = None
    invalid_params: Optional[List[Dict[str, Any]]] = None


class PdfCraftError(Exception):
    """Base exception for all PdfCraft SDK operations."""

    def __init__(self, message: str, status_code: int = 500, error_code: str = "ENGINE_ERROR"):
        super().__init__(message)
        self.message = message
        self.status_code = status_code
        self.error_code = error_code


class InvalidArgumentError(PdfCraftError):
    """Raised when an argument is missing, empty, or violates domain rules."""

    def __init__(self, message: str):
        super().__init__(message, status_code=400, error_code="INVALID_ARGUMENT")


class NotFoundError(PdfCraftError):
    """Raised when an input document or job is not found."""

    def __init__(self, message: str):
        super().__init__(message, status_code=404, error_code="NOT_FOUND")


class ConflictError(PdfCraftError):
    """Raised when attempting an invalid state transition or operation."""

    def __init__(self, message: str):
        super().__init__(message, status_code=409, error_code="CONFLICT")


class QuotaExceededError(PdfCraftError):
    """Raised when document limits or execution concurrency are saturated."""

    def __init__(self, message: str):
        super().__init__(message, status_code=413, error_code="QUOTA_EXCEEDED")
