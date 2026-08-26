pub const EMBEDDING_DIM: usize = 384;
pub trait Embedder: Send + Sync {
    fn embed(&self, text: &str) -> Vec<f32>;
    fn dim(&self) -> usize {
        EMBEDDING_DIM
    }
}
#[derive(Clone, Default)]
pub struct HashedEmbedder;
impl Embedder for HashedEmbedder {
    fn embed(&self, text: &str) -> Vec<f32> {
        let mut out = vec![0.0; EMBEDDING_DIM];
        for token in text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| t.len() >= 2)
        {
            let h = fnv1a(token.to_lowercase().as_bytes());
            out[h as usize % EMBEDDING_DIM] += 1.0;
            out[(h.rotate_right(17) as usize) % EMBEDDING_DIM] += 1.0;
        }
        let norm = out.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > f32::EPSILON {
            for v in &mut out {
                *v /= norm;
            }
        } else {
            out[0] = 1.0;
        }
        out
    }
}
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub(crate) fn vector_json(v: &[f32]) -> String {
    let mut s = String::from("[");
    for (i, n) in v.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&n.to_string());
    }
    s.push(']');
    s
}
