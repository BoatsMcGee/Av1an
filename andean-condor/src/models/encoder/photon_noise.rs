use std::hash::{DefaultHasher, Hash, Hasher};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PhotonNoise {
    pub iso:        u32,
    pub chroma_iso: Option<u32>,
    pub width:      Option<u32>,
    pub height:     Option<u32>,
    pub c_y:        Option<Vec<i8>>,
    pub ccb:        Option<Vec<i8>>,
    pub ccr:        Option<Vec<i8>>,
}

impl Hash for PhotonNoise {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        serde_json::to_vec(self).expect("PhotonNoise should serialize").hash(state);
    }
}

impl PhotonNoise {
    #[inline]
    pub fn hash_name(&self) -> String {
        PhotonNoise::hash_full(self)
    }

    #[inline]
    pub fn hash_full(photon_noise: &PhotonNoise) -> String {
        let mut hasher = DefaultHasher::new();
        photon_noise.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::PhotonNoise;

    fn photon_noise(iso: u32) -> PhotonNoise {
        PhotonNoise {
            iso,
            chroma_iso: Some(iso / 3),
            width: None,
            height: None,
            c_y: None,
            ccb: None,
            ccr: None,
        }
    }

    /// Value equality must match the manual `Hash` (which serializes), so a
    /// re-encoded config compares equal to the one it came from.
    #[test]
    fn photon_noise_compares_by_value() {
        assert_eq!(photon_noise(600), photon_noise(600));
        assert_ne!(photon_noise(600), photon_noise(12000));
    }

    #[test]
    fn a_chroma_iso_difference_is_a_difference() {
        let mut scaled = photon_noise(600);
        scaled.chroma_iso = Some(4000);
        assert_ne!(photon_noise(600), scaled);
    }
}
