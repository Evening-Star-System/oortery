//! Stupid-easy presets for the most common object stores. rclone's raw S3 form asks for a provider
//! (dozens of choices), an endpoint, a region, and keys, which is intimidating. A preset asks only
//! the essentials and assembles the real rclone config, so connecting Cloudflare R2 or any S3 bucket
//! is a short, obvious form. S3-style backends do not log in at create time, so a correct preset
//! connects first try.

use std::collections::BTreeMap;

/// One field a preset asks the user for.
pub struct PresetField {
    pub key: &'static str,
    pub label: &'static str,
    pub secret: bool,
    pub required: bool,
}

/// A curated connection preset over an rclone backend.
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub blurb: &'static str,
    pub fields: &'static [PresetField],
}

const ACCESS_KEY: PresetField = PresetField {
    key: "access_key_id",
    label: "Access Key ID",
    secret: false,
    required: true,
};
const SECRET_KEY: PresetField = PresetField {
    key: "secret_access_key",
    label: "Secret Access Key",
    secret: true,
    required: true,
};

/// The presets shown at the top of the add-remote picker, in order.
pub const PRESETS: &[Preset] = &[
    Preset {
        id: "r2",
        name: "Cloudflare R2",
        blurb: "Cloudflare R2 object storage",
        fields: &[
            PresetField {
                key: "account_id",
                label: "Account ID",
                secret: false,
                required: true,
            },
            ACCESS_KEY,
            SECRET_KEY,
        ],
    },
    Preset {
        id: "s3",
        name: "Amazon S3",
        blurb: "Amazon Web Services S3",
        fields: &[
            ACCESS_KEY,
            SECRET_KEY,
            PresetField {
                key: "region",
                label: "Region (e.g. us-east-1)",
                secret: false,
                required: false,
            },
        ],
    },
    Preset {
        id: "s3compat",
        name: "Other S3-compatible",
        blurb: "Any S3-compatible storage (MinIO, Wasabi, DigitalOcean, ...)",
        fields: &[
            PresetField {
                key: "endpoint",
                label: "Endpoint URL",
                secret: false,
                required: true,
            },
            ACCESS_KEY,
            SECRET_KEY,
            PresetField {
                key: "region",
                label: "Region (optional)",
                secret: false,
                required: false,
            },
        ],
    },
    Preset {
        id: "b2",
        name: "Backblaze B2",
        blurb: "Backblaze B2 cloud storage",
        fields: &[
            PresetField {
                key: "account",
                label: "Key ID",
                secret: false,
                required: true,
            },
            PresetField {
                key: "key",
                label: "Application Key",
                secret: true,
                required: true,
            },
        ],
    },
];

/// Look up a preset by id.
pub fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// Assemble the rclone backend id and config params for a preset from the user's field values.
/// Returns `None` for an unknown preset id. Empty optional fields are dropped so rclone uses its
/// default (e.g. no region).
pub fn build(
    id: &str,
    values: &BTreeMap<String, String>,
) -> Option<(String, Vec<(String, String)>)> {
    let get = |k: &str| values.get(k).cloned().unwrap_or_default();
    let mut out: Vec<(String, String)> = Vec::new();
    let mut push = |k: &str, v: String| {
        if !v.is_empty() {
            out.push((k.to_string(), v));
        }
    };
    let backend = match id {
        "r2" => {
            push("provider", "Cloudflare".into());
            push("access_key_id", get("access_key_id"));
            push("secret_access_key", get("secret_access_key"));
            push(
                "endpoint",
                format!(
                    "https://{}.r2.cloudflarestorage.com",
                    get("account_id").trim()
                ),
            );
            push("region", "auto".into());
            "s3"
        }
        "s3" => {
            push("provider", "AWS".into());
            push("access_key_id", get("access_key_id"));
            push("secret_access_key", get("secret_access_key"));
            push("region", get("region"));
            "s3"
        }
        "s3compat" => {
            push("provider", "Other".into());
            push("access_key_id", get("access_key_id"));
            push("secret_access_key", get("secret_access_key"));
            push("endpoint", get("endpoint"));
            push("region", get("region"));
            "s3"
        }
        "b2" => {
            push("account", get("account"));
            push("key", get("key"));
            "b2"
        }
        _ => return None,
    };
    Some((backend.to_string(), out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r2_builds_the_endpoint_from_account_id() {
        let mut v = BTreeMap::new();
        v.insert("account_id".to_string(), "abc123".to_string());
        v.insert("access_key_id".to_string(), "AK".to_string());
        v.insert("secret_access_key".to_string(), "SK".to_string());
        let (backend, params) = build("r2", &v).unwrap();
        assert_eq!(backend, "s3");
        assert!(params.contains(&("provider".into(), "Cloudflare".into())));
        assert!(params.contains(&(
            "endpoint".into(),
            "https://abc123.r2.cloudflarestorage.com".into()
        )));
        assert!(params.contains(&("region".into(), "auto".into())));
        assert!(params.contains(&("access_key_id".into(), "AK".into())));
    }

    #[test]
    fn s3compat_drops_empty_region() {
        let mut v = BTreeMap::new();
        v.insert("endpoint".to_string(), "https://minio.example".to_string());
        v.insert("access_key_id".to_string(), "AK".to_string());
        v.insert("secret_access_key".to_string(), "SK".to_string());
        // region left out
        let (_backend, params) = build("s3compat", &v).unwrap();
        assert!(params.iter().all(|(k, _)| k != "region"));
        assert!(params.contains(&("endpoint".into(), "https://minio.example".into())));
    }

    #[test]
    fn unknown_preset_is_none() {
        assert!(build("nope", &BTreeMap::new()).is_none());
    }
}
