//! Deterministic property checks for `snap::geometry` and `Geometry::snap`: mutated synthetic
//! content keeps extracted geometry bounded and finite, and snap queries return finite results.

use std::sync::Arc;

use pdfcraft_cos::Document;
use pdfcraft_measure::snap::{self, Geometry, SnapOptions};

const CASES: usize = 1_000;
const SEED: u64 = 1;
const MAX_CONTENT_BYTES: usize = 16 * 1024;
const MAX_GEOMETRY_SEGMENTS: usize = 20_000;
const MAX_SNAP_TARGETS: usize = 2 * MAX_GEOMETRY_SEGMENTS + 12;

const SEEDS: &[&[u8]] = &[
    b"0 0 m 100 100 l S",
    b"q 1 0 0 1 10 20 cm 0 0 10 10 re S Q",
    b"0 0 m 20 100 80 -100 100 0 c S",
    b"1e308 1e308 m -1e308 -1e308 l S",
    b"BI /W 1 /H 1 /BPC 8 /CS /RGB ID \x00\xff\x7f EI",
    b"q q q Q Q cm m l c v y h re S n",
];
const TOKENS: &[&[u8]] = &[
    b" m ",
    b" l ",
    b" c ",
    b" v ",
    b" y ",
    b" re ",
    b" S ",
    b" f* ",
    b" q Q ",
    b" cm ",
    b" Do ",
    b" BI ID EI ",
    b" 1e308 ",
    b" -1e308 ",
    b" /BadName ",
    b" [1 2 (unterminated] ",
];
const DENSE_PATH: &[u8] = b"0 0 m 1 1 l S ";

#[derive(Clone)]
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

fn mutate(rng: &mut Rng, seed: &[u8]) -> Vec<u8> {
    let mut data = seed.to_vec();
    let rounds = 1 + rng.below(8);
    for _ in 0..rounds {
        if data.is_empty() {
            data.extend_from_slice(b"0 0 m 10 10 l S");
        }
        let at = rng.below(data.len());
        match rng.below(6) {
            0 => data[at] ^= 1 << rng.below(8),
            1 => {
                let n = 1 + rng.below(64.min(data.len() - at));
                data.drain(at..at + n);
            }
            2 => {
                let token = TOKENS[rng.below(TOKENS.len())];
                data.splice(at..at, token.iter().copied());
            }
            3 => {
                let n = 1 + rng.below(128.min(data.len() - at));
                let duplicate = data[at..at + n].to_vec();
                let to = rng.below(data.len());
                data.splice(to..to, duplicate);
            }
            4 => {
                let byte = [rng.next() as u8];
                data.splice(at..at, byte);
            }
            _ => {
                let token = TOKENS[rng.below(TOKENS.len())];
                let end = at.saturating_add(1).min(data.len());
                data.splice(at..end, token.iter().copied());
            }
        }
        data.truncate(MAX_CONTENT_BYTES);
    }
    data
}

fn one_page_pdf(content: &[u8]) -> Vec<u8> {
    let stream = format!("<< /Length {} >>\nstream\n", content.len());
    let objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> >>".to_vec(),
        {
            let mut object = stream.into_bytes();
            object.extend_from_slice(content);
            object.extend_from_slice(b"\nendstream");
            object
        },
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        bytes.extend_from_slice(object);
        bytes.extend_from_slice(b"\nendobj\n");
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    bytes
}

fn finite(point: [f64; 2]) -> bool {
    point.into_iter().all(f64::is_finite)
}

fn geometry_of(content: &[u8]) -> Geometry {
    let doc = Document::open(Arc::new(one_page_pdf(content))).unwrap();
    snap::geometry(&doc, 0).unwrap()
}

/// Checks the bounds and finiteness that snapping relies on, and returns the extracted geometry.
fn check_snap_invariants(content: &[u8]) -> Geometry {
    assert!(content.len() <= MAX_CONTENT_BYTES, "generated content exceeded the byte cap");
    let geometry = geometry_of(content);
    assert!(geometry.segments.len() <= MAX_GEOMETRY_SEGMENTS, "geometry exceeded the segment cap: {}", geometry.segments.len());
    assert!(geometry.endpoints.len().saturating_add(geometry.midpoints.len()) <= MAX_SNAP_TARGETS, "geometry exceeded the snap-target cap");
    assert!(geometry.segments.iter().flatten().copied().all(finite), "geometry contains a non-finite segment coordinate");
    assert!(geometry.endpoints.iter().copied().all(finite), "geometry contains a non-finite endpoint");
    assert!(geometry.midpoints.iter().copied().all(finite), "geometry contains a non-finite midpoint");

    geometry.intersection_limited([0.0, 0.0], 10_000.0);
    let snap = geometry.snap([0.0, 0.0], 10_000.0, SnapOptions::default()).unwrap();
    if let Some(hit) = snap {
        assert!(finite(hit.point) && hit.distance.is_finite(), "snap result contains a non-finite value");
    }
    geometry
}

#[test]
fn mutated_snap_geometry_stays_bounded_and_finite() {
    let mut seeds: Vec<Vec<u8>> = SEEDS.iter().map(|seed| seed.to_vec()).collect();
    let mut dense = Vec::with_capacity(MAX_CONTENT_BYTES);
    while dense.len().saturating_add(DENSE_PATH.len()) <= MAX_CONTENT_BYTES {
        dense.extend_from_slice(DENSE_PATH);
    }
    seeds.push(dense);

    let mut rng = Rng(SEED);
    for _ in 0..CASES {
        let seed_index = rng.below(seeds.len());
        let content = mutate(&mut rng, &seeds[seed_index]);
        check_snap_invariants(&content);
    }
}

#[test]
fn high_curvature_geometry_hits_segment_cap() {
    let stress_curve = b"0 0 m 0 100000000 100000000 100000000 100000000 0 c S ".repeat(6);
    let geometry = check_snap_invariants(&stress_curve);
    assert!(geometry.truncated, "high-curvature geometry did not report its extraction cap ({} segments)", geometry.segments.len());
}
