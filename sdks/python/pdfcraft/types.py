"""Types and data models for the PdfCraft Python SDK (Issue #872).

# Architecture Reference
Module: sdks/python/pdfcraft/types.py
Purpose:
    Strongly typed dataclasses mirroring the Datalogics APDFL 21 domain models
    and Rust SDK contracts in pure Python.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum, auto
from pathlib import Path
from typing import Any, Dict, List, Optional, Union


# ==============================================================================
# Domain 1: Core Value Objects & Options
# ==============================================================================

@dataclass
class DocumentSource:
    """Input abstraction decoupling operations from raw filesystem paths."""

    path: Optional[Path] = None
    data: Optional[bytes] = None
    name: Optional[str] = None

    @classmethod
    def from_path(cls, path: Union[str, Path]) -> DocumentSource:
        """Creates a DocumentSource referencing a local file path."""
        return cls(path=Path(path))

    @classmethod
    def from_bytes(cls, data: bytes, name: Optional[str] = None) -> DocumentSource:
        """Creates a DocumentSource referencing an in-memory byte buffer."""
        return cls(data=data, name=name)


@dataclass
class DocumentResult:
    """Output abstraction representing in-memory or spilled file results."""

    data: Optional[bytes] = None
    path: Optional[Path] = None
    mime_type: str = "application/pdf"
    size_bytes: int = 0

    @property
    def is_memory(self) -> bool:
        """Returns True if the output is held in memory."""
        return self.data is not None

    @property
    def is_file(self) -> bool:
        """Returns True if the output was written to a file on disk."""
        return self.path is not None


@dataclass
class MergeOptions:
    """Configuration options for merging documents."""

    output_filename: Optional[str] = None
    pages: Optional[List[Optional[str]]] = None


class SplitModeType(Enum):
    EVERY_N_PAGES = auto()
    BEFORE_PAGES = auto()
    AT_BOOKMARKS = auto()
    MAX_FILE_SIZE_MB = auto()


@dataclass
class SplitMode:
    """Partitioning strategy for document splitting."""

    mode: SplitModeType
    every_n_pages: Optional[int] = None
    before_pages: Optional[List[int]] = None
    max_file_size_mb: Optional[float] = None

    @classmethod
    def every_n(cls, n: int) -> SplitMode:
        return cls(mode=SplitModeType.EVERY_N_PAGES, every_n_pages=n)


@dataclass
class SplitResult:
    """Result of a split operation containing generated output file paths."""

    files: List[Path] = field(default_factory=list)


class ImageFormat(str, Enum):
    """Target image raster format."""

    PNG = "Png"
    JPEG = "Jpeg"
    WEBP = "Webp"
    TIFF = "Tiff"


@dataclass
class RenderOptions:
    """Rendering configuration calculating physical DPI."""

    dpi: float = 150.0
    format: ImageFormat = ImageFormat.PNG


# ==============================================================================
# Domain 2: Graphics State & Geometry
# ==============================================================================

@dataclass
class Point:
    """2D coordinate in PDF user space."""

    x: float = 0.0
    y: float = 0.0


@dataclass
class Rect:
    """Bounding box rectangle in PDF user space."""

    x: float = 0.0
    y: float = 0.0
    width: float = 0.0
    height: float = 0.0

    @property
    def right(self) -> float:
        return self.x + self.width

    @property
    def top(self) -> float:
        return self.y + self.height


@dataclass
class Quad:
    """Quadrilateral defined by four corner vertices."""

    top_left: Point = field(default_factory=Point)
    top_right: Point = field(default_factory=Point)
    bottom_left: Point = field(default_factory=Point)
    bottom_right: Point = field(default_factory=Point)

    @classmethod
    def from_rect(cls, rect: Rect) -> Quad:
        return cls(
            top_left=Point(rect.x, rect.y + rect.height),
            top_right=Point(rect.x + rect.width, rect.y + rect.height),
            bottom_left=Point(rect.x, rect.y),
            bottom_right=Point(rect.x + rect.width, rect.y),
        )


@dataclass
class Matrix:
    """3x2 Affine transform matrix [a, b, c, d, tx, ty]."""

    a: float = 1.0
    b: float = 0.0
    c: float = 0.0
    d: float = 1.0
    tx: float = 0.0
    ty: float = 0.0

    @classmethod
    def identity(cls) -> Matrix:
        return cls()


@dataclass
class Color:
    """PDF Color value."""

    r: float = 0.0
    g: float = 0.0
    b: float = 0.0

    @classmethod
    def black(cls) -> Color:
        return cls(0.0, 0.0, 0.0)

    @classmethod
    def white(cls) -> Color:
        return cls(1.0, 1.0, 1.0)

    @classmethod
    def rgb(cls, r: float, g: float, b: float) -> Color:
        return cls(r, g, b)

    def __str__(self) -> str:
        return f"RGB({self.r}, {self.g}, {self.b})"


class ColorSpace(str, Enum):
    DEVICE_RGB = "DeviceRGB"
    DEVICE_CMYK = "DeviceCMYK"
    DEVICE_GRAY = "DeviceGray"
    ICC_BASED = "ICCBased"


class BlendMode(str, Enum):
    NORMAL = "Normal"
    MULTIPLY = "Multiply"
    SCREEN = "Screen"
    OVERLAY = "Overlay"


# ==============================================================================
# Domain 3: Document Layer
# ==============================================================================

class FitMode(str, Enum):
    XYZ = "XYZ"
    FIT = "Fit"
    FIT_H = "FitH"
    FIT_V = "FitV"
    FIT_R = "FitR"


@dataclass
class ViewDestination:
    """Target view destination within a document."""

    page_number: int
    fit_mode: FitMode = FitMode.FIT
    zoom: Optional[float] = None
    rect: Optional[Rect] = None


@dataclass
class Bookmark:
    """Outline bookmark node."""

    title: str
    destination: Optional[ViewDestination] = None
    children: List[Bookmark] = field(default_factory=list)
    is_open: bool = True


@dataclass
class Page:
    """Page geometry and metadata."""

    page_number: int
    media_box: Rect
    crop_box: Rect = field(default_factory=Rect)
    rotation_degrees: int = 0


@dataclass
class Document:
    """Document layer root descriptor."""

    num_pages: int
    version: str = "1.7"
    is_encrypted: bool = False
    linearized: bool = False
    bookmarks: List[Bookmark] = field(default_factory=list)


# ==============================================================================
# Domain 4: Text, Fonts & Styles
# ==============================================================================

@dataclass
class Style:
    """Typographical style data (font, font size, color) used in a word."""

    font_name: str
    font_size: float
    color: Color = field(default_factory=Color.black)

    def __str__(self) -> str:
        return f"[color={self.color}, fontsize={self.font_size}, fontname={self.font_name}]"


@dataclass
class StyleTransition:
    """Represents a style change at a specific character offset in a word."""

    char_index: int
    style: Style


@dataclass
class Word:
    """Extracted word token with spatial bounding coordinates and styles."""

    text: str
    bounding_box: Rect
    quads: List[Quad] = field(default_factory=list)
    styles: List[StyleTransition] = field(default_factory=list)


@dataclass
class WordFinderConfig:
    """Configuration options for the WordFinder algorithm."""

    preserve_ligatures: bool = False
    precise_quads: bool = True
    decompose_hyphenated_words: bool = True


@dataclass
class DocTextFinderConfig:
    """Configuration options for text pattern searching."""

    case_sensitive: bool = False
    whole_words_only: bool = False
    regex: bool = False


@dataclass
class DocTextFinderMatch:
    """Hit result from document text search."""

    page_number: int
    matched_text: str
    bounding_box: Rect
    quads: List[Quad] = field(default_factory=list)


# ==============================================================================
# Domain 5: Annotations & Actions
# ==============================================================================

@dataclass
class GoToAction:
    """Internal jump action navigating to a view destination in the same document."""

    destination: ViewDestination


@dataclass
class URIAction:
    """Web hyperlink action."""

    uri: str
    is_map: bool = False


@dataclass
class Action:
    """Base polymorphic action."""

    goto: Optional[GoToAction] = None
    uri: Optional[URIAction] = None


@dataclass
class HighlightAnnotation:
    """Highlight text markup annotation."""

    quads: List[Quad]
    color: Color = field(default_factory=lambda: Color.rgb(1.0, 1.0, 0.0))


@dataclass
class Redaction:
    """Redaction annotation marking content for removal."""

    quads: List[Quad]
    overlay_text: Optional[str] = None
    fill_color: Color = field(default_factory=Color.black)


@dataclass
class Annotation:
    """Base interactive annotation descriptor."""

    rect: Rect
    contents: Optional[str] = None
    color: Optional[Color] = None
    opacity: float = 1.0
    action: Optional[Action] = None
    highlight: Optional[HighlightAnnotation] = None
    redaction: Optional[Redaction] = None


# ==============================================================================
# Domain 6: Forms & Data Exchange
# ==============================================================================

class AcroFormExportType(int, Enum):
    FDF = 1
    XFDF = 2
    XML = 3
    JSON = 4


@dataclass
class TextField:
    multiline: bool = False
    password: bool = False


@dataclass
class ButtonField:
    is_checkbox: bool = False
    is_radio: bool = False
    checked: bool = False


@dataclass
class Field:
    """Interactive form field descriptor."""

    name: str
    page_number: int
    rect: Rect
    value: Optional[str] = None
    text_field: Optional[TextField] = None
    button_field: Optional[ButtonField] = None


# ==============================================================================
# Domain 7: Digital Signatures & Security
# ==============================================================================

@dataclass
class SignDoc:
    """Digital signature specification."""

    cert_p12_bytes: bytes
    password: Optional[str] = None
    signer_name: Optional[str] = None
    reason: Optional[str] = None
    location: Optional[str] = None


@dataclass
class PermissionsFlags:
    """Document usage permissions bitmask."""

    allow_print: bool = True
    allow_modify: bool = True
    allow_copy: bool = True
    allow_annotate: bool = True


# ==============================================================================
# Domain 8: PDF Optimization
# ==============================================================================

@dataclass
class OptimizationParams:
    """PDF compression and cleanup settings."""

    compress_streams: bool = True
    remove_unreferenced_objects: bool = True
    downsample_images: bool = False
    linearize: bool = False


@dataclass
class PDFOptimizer:
    params: OptimizationParams = field(default_factory=OptimizationParams)
