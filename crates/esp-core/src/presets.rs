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
    /// A one-line subtitle for the picker row.
    pub blurb: &'static str,
    /// A plain-language note with the real gotchas (cost, region, where to click), shown on the form.
    pub help: &'static str,
    /// A numbered, handheld walkthrough of getting the bucket + keys, shown on the form.
    pub steps: &'static [&'static str],
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
/// The one bucket this connection targets. Required because the keys we tell users to create are
/// scoped to a single bucket: such a key cannot list the account's buckets, so Oortery must know
/// which bucket to point at (both to verify the connection and to land browsing/syncs there).
const BUCKET: PresetField = PresetField {
    key: "bucket",
    label: "Bucket name",
    secret: false,
    required: true,
};

/// The presets shown at the top of the add-remote picker, in order.
pub const PRESETS: &[Preset] = &[
    Preset {
        id: "r2",
        name: "Cloudflare R2",
        blurb: "Cloudflare R2 object storage",
        help: "Free within generous limits, with no egress fees. Cloudflare may ask for a card to \
               switch R2 on; there is no charge while you stay under the free tier. Region stays \
               \u{201c}auto\u{201d}.",
        steps: &[
            "In the Cloudflare dashboard, open R2 and choose Create bucket. Use that name in the Bucket field below.",
            "Open the bucket's Settings and paste its whole S3 API endpoint into Account ID below (or just the account part before \u{201c}.r2.cloudflarestorage.com\u{201d}); Oortery figures out the rest.",
            "Back in R2, open Manage R2 API Tokens and create an Account API token with Object Read & Write, scoped to just this one bucket.",
            "Copy the Access Key ID and Secret Access Key into the fields below. The secret is shown only once.",
        ],
        fields: &[
            PresetField {
                key: "account_id",
                label: "Account ID (or paste the S3 endpoint)",
                secret: false,
                required: true,
            },
            BUCKET,
            ACCESS_KEY,
            SECRET_KEY,
        ],
    },
    Preset {
        id: "s3",
        name: "Amazon S3",
        blurb: "Amazon Web Services S3",
        help: "Use an IAM key scoped to just this bucket, so a lifted key cannot reach the rest of \
               your AWS account. Set the region to match where you create the bucket.",
        steps: &[
            "In AWS S3, create a private bucket and note its region.",
            "Set the Region field below to match the bucket's region.",
            "Create an IAM user/key limited to that bucket (List, Get, Put, Delete object).",
            "Copy its Access Key ID and Secret Access Key below. The secret is shown only once.",
        ],
        fields: &[
            BUCKET,
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
        help: "Works with any S3-compatible store. Paste the exact endpoint URL from your provider, \
               and match the region to your bucket (leave it blank if your provider does not use one). \
               Prefer a key scoped to just this bucket.",
        steps: &[
            "Create a private bucket with your provider and note its endpoint URL and region.",
            "Paste the full endpoint URL below (for example https://s3.us-east-1.wasabisys.com).",
            "Set the region to match, or leave it blank if the provider has none.",
            "Create an access key scoped to that bucket and copy the ID and secret below.",
        ],
        fields: &[
            PresetField {
                key: "endpoint",
                label: "Endpoint URL",
                secret: false,
                required: true,
            },
            BUCKET,
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
        help: "Cheapest storage per TB. Use an application key restricted to one bucket. The Key ID \
               is the account field; the application key is the secret.",
        steps: &[
            "In Backblaze, open Buckets and create a private bucket.",
            "Open App Keys, Add a New Application Key restricted to that bucket, Read and Write.",
            "Copy the keyID into Key ID below, and the applicationKey into Application Key.",
            "The application key is shown only once; copy it before you leave that page.",
        ],
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
            BUCKET,
        ],
    },
];

/// Look up a preset by id.
pub fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// What `build` produces: the rclone backend id, its config params, and the target bucket (may be
/// empty for a preset without one).
pub type PresetBuild = (String, Vec<(String, String)>, String);

/// Extract a Cloudflare R2 account id from whatever the user pasted: a bare id, or the full S3 API
/// endpoint they were already looking at (`https://<id>.r2.cloudflarestorage.com`). Paste-smart, so
/// a user does not have to hand-trim the one string Cloudflare shows them.
pub fn r2_account_id(input: &str) -> String {
    let s = input.trim();
    if let Some(idx) = s.find(".r2.cloudflarestorage.com") {
        let head = &s[..idx];
        return head.rsplit("://").next().unwrap_or(head).to_string();
    }
    s.to_string()
}

/// Best-effort parse of an AWS-style region label from an S3 endpoint host, e.g.
/// `https://s3.us-east-1.wasabisys.com` -> `us-east-1`, so an S3-compatible connection can fill the
/// region the user already encoded in their endpoint. `None` when no region-shaped label is present.
pub fn region_from_endpoint(endpoint: &str) -> Option<String> {
    let host = endpoint.trim().rsplit("://").next().unwrap_or(endpoint);
    let host = host.split('/').next().unwrap_or(host);
    for label in host.split('.') {
        let parts: Vec<&str> = label.split('-').collect();
        if parts.len() == 3
            && (2..=3).contains(&parts[0].len())
            && parts[0].chars().all(|c| c.is_ascii_lowercase())
            && !parts[2].is_empty()
            && parts[2].chars().all(|c| c.is_ascii_digit())
        {
            return Some(label.to_string());
        }
    }
    None
}

/// Assemble the rclone backend id, config params, and target bucket for a preset from the user's
/// field values. Returns `None` for an unknown preset id. Empty optional fields are dropped so
/// rclone uses its default (e.g. no region). The bucket is returned separately (not a config key):
/// object stores address a bucket by path, so Oortery stores it as the remote's landing root and
/// verifies against it. The bucket string is trimmed and may be empty for presets without one.
pub fn build(id: &str, values: &BTreeMap<String, String>) -> Option<PresetBuild> {
    let get = |k: &str| values.get(k).cloned().unwrap_or_default();
    let bucket = get("bucket").trim().to_string();
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
                    r2_account_id(&get("account_id"))
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
            // Use the region the user typed, else derive it from the endpoint they pasted.
            let region = {
                let r = get("region");
                if r.trim().is_empty() {
                    region_from_endpoint(&get("endpoint")).unwrap_or_default()
                } else {
                    r
                }
            };
            push("region", region);
            "s3"
        }
        "b2" => {
            push("account", get("account"));
            push("key", get("key"));
            "b2"
        }
        _ => return None,
    };
    Some((backend.to_string(), out, bucket))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r2_builds_the_endpoint_from_account_id() {
        let mut v = BTreeMap::new();
        v.insert("account_id".to_string(), "abc123".to_string());
        v.insert("bucket".to_string(), "  my-bucket  ".to_string());
        v.insert("access_key_id".to_string(), "AK".to_string());
        v.insert("secret_access_key".to_string(), "SK".to_string());
        let (backend, params, bucket) = build("r2", &v).unwrap();
        assert_eq!(backend, "s3");
        assert!(params.contains(&("provider".into(), "Cloudflare".into())));
        assert!(params.contains(&(
            "endpoint".into(),
            "https://abc123.r2.cloudflarestorage.com".into()
        )));
        assert!(params.contains(&("region".into(), "auto".into())));
        assert!(params.contains(&("access_key_id".into(), "AK".into())));
        // The bucket comes back trimmed and separate, never as a config param.
        assert_eq!(bucket, "my-bucket");
        assert!(params.iter().all(|(k, _)| k != "bucket"));
    }

    #[test]
    fn s3compat_drops_empty_region() {
        let mut v = BTreeMap::new();
        v.insert("endpoint".to_string(), "https://minio.example".to_string());
        v.insert("access_key_id".to_string(), "AK".to_string());
        v.insert("secret_access_key".to_string(), "SK".to_string());
        // region left out
        let (_backend, params, _bucket) = build("s3compat", &v).unwrap();
        assert!(params.iter().all(|(k, _)| k != "region"));
        assert!(params.contains(&("endpoint".into(), "https://minio.example".into())));
    }

    #[test]
    fn unknown_preset_is_none() {
        assert!(build("nope", &BTreeMap::new()).is_none());
    }

    #[test]
    fn r2_account_id_accepts_a_pasted_endpoint() {
        assert_eq!(r2_account_id("abc123"), "abc123");
        assert_eq!(
            r2_account_id("https://abc123.r2.cloudflarestorage.com"),
            "abc123"
        );
        assert_eq!(
            r2_account_id("  abc123.r2.cloudflarestorage.com/mybucket "),
            "abc123"
        );
    }

    #[test]
    fn region_parsed_from_endpoint_host() {
        assert_eq!(
            region_from_endpoint("https://s3.us-east-1.wasabisys.com").as_deref(),
            Some("us-east-1")
        );
        assert_eq!(
            region_from_endpoint("https://s3.ap-southeast-2.amazonaws.com").as_deref(),
            Some("ap-southeast-2")
        );
        // A plain MinIO host has no region label.
        assert_eq!(region_from_endpoint("https://minio.example.com"), None);
    }

    #[test]
    fn s3compat_fills_region_from_endpoint_when_blank() {
        let mut v = BTreeMap::new();
        v.insert(
            "endpoint".to_string(),
            "https://s3.us-west-1.wasabisys.com".to_string(),
        );
        v.insert("bucket".to_string(), "b".to_string());
        v.insert("access_key_id".to_string(), "AK".to_string());
        v.insert("secret_access_key".to_string(), "SK".to_string());
        let (_b, params, _bk) = build("s3compat", &v).unwrap();
        assert!(params.contains(&("region".into(), "us-west-1".into())));
    }
}
