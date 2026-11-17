//! The opendal services the picker offers, with the named fields of each (ADR-0009). The keys
//! are opendal's config keys; a test checks them against opendal's config structs.

/// One named field of an opendal service in the connect form. `key` is the opendal option key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendField {
    /// The option key stored in the config.
    pub key: &'static str,
    /// The label in the form.
    pub label: &'static str,
    /// Other keys opendal reads as this one. An option stored under one counts as this field's.
    pub aliases: &'static [&'static str],
    /// Whether the service cannot be built without a value and has no fallback to the
    /// environment.
    pub required: bool,
    /// Whether a new value is kept in the keyring instead of the config.
    pub secret: bool,
    /// Placeholder text for the empty field; empty for none.
    pub example: &'static str,
    /// One line under the field; empty for none.
    pub hint: &'static str,
}

impl BackendField {
    const fn new(key: &'static str, label: &'static str) -> Self {
        Self {
            key,
            label,
            aliases: &[],
            required: false,
            secret: false,
            example: "",
            hint: "",
        }
    }

    const fn aliases(mut self, aliases: &'static [&'static str]) -> Self {
        self.aliases = aliases;
        self
    }

    const fn required(mut self) -> Self {
        self.required = true;
        self
    }

    const fn secret(mut self) -> Self {
        self.secret = true;
        self
    }

    const fn example(mut self, example: &'static str) -> Self {
        self.example = example;
        self
    }

    const fn hint(mut self, hint: &'static str) -> Self {
        self.hint = hint;
        self
    }
}

/// An opendal service as the picker offers it.
pub(super) struct Service {
    /// The opendal scheme, as in `opendal:<name>`.
    pub name: &'static str,
    /// The name shown in the picker.
    pub label: &'static str,
    /// The named fields, `root` last.
    pub fields: &'static [BackendField],
}

/// The folder every service has. Not "Root folder": Settings has a field of that name.
const ROOT: BackendField = BackendField::new("root", "Folder")
    .example("/salvajocs")
    .hint("Folder inside the storage; the repository goes here.");

/// The fields of the OAuth services: either an access token, or a refresh token with the client.
const OAUTH: &[BackendField] = &[
    BackendField::new("access_token", "Access token")
        .secret()
        .hint("Or a refresh token with a client ID and secret."),
    BackendField::new("refresh_token", "Refresh token").secret(),
    BackendField::new("client_id", "Client ID"),
    BackendField::new("client_secret", "Client secret").secret(),
    ROOT,
];

/// The opendal services offered in the picker: those `rustic_backend` builds for this OS, minus
/// the ones ADR-0009 hides. `sftp` is Unix-only (ADR-0003).
pub(super) const OPENDAL_SERVICES: &[Service] = &[
    Service {
        name: "azblob",
        label: "Azure Blob Storage",
        fields: &[
            BackendField::new("container", "Container")
                .aliases(&["azure_container_name", "container_name"])
                .required()
                .example("backups"),
            BackendField::new("endpoint", "Endpoint")
                .aliases(&["azure_storage_endpoint", "azure_endpoint"])
                .required()
                .example("https://myaccount.blob.core.windows.net"),
            BackendField::new("account_name", "Account name"),
            BackendField::new("account_key", "Account key").secret(),
            BackendField::new("sas_token", "SAS token")
                .secret()
                .hint("Instead of the account key."),
            ROOT,
        ],
    },
    Service {
        name: "azdls",
        label: "Azure Data Lake Storage",
        fields: &[
            BackendField::new("filesystem", "Filesystem").required(),
            BackendField::new("endpoint", "Endpoint")
                .required()
                .example("https://myaccount.dfs.core.windows.net"),
            BackendField::new("account_name", "Account name"),
            BackendField::new("account_key", "Account key").secret(),
            ROOT,
        ],
    },
    Service {
        name: "azfile",
        label: "Azure Files",
        fields: &[
            BackendField::new("share_name", "Share name").required(),
            BackendField::new("endpoint", "Endpoint")
                .required()
                .example("https://myaccount.file.core.windows.net"),
            BackendField::new("account_name", "Account name")
                .hint("Read from an Azure endpoint when empty."),
            BackendField::new("account_key", "Account key").secret(),
            ROOT,
        ],
    },
    Service {
        name: "b2",
        label: "Backblaze B2",
        fields: &[
            BackendField::new("bucket", "Bucket").required(),
            BackendField::new("bucket_id", "Bucket ID").required(),
            BackendField::new("application_key_id", "Application key ID").required(),
            BackendField::new("application_key", "Application key")
                .required()
                .secret(),
            ROOT,
        ],
    },
    Service {
        name: "cos",
        label: "Tencent Cloud COS",
        fields: &[
            BackendField::new("bucket", "Bucket").required(),
            BackendField::new("endpoint", "Endpoint")
                .required()
                .example("https://cos.ap-guangzhou.myqcloud.com"),
            BackendField::new("secret_id", "Secret ID"),
            BackendField::new("secret_key", "Secret key").secret(),
            ROOT,
        ],
    },
    Service {
        name: "dropbox",
        label: "Dropbox",
        fields: OAUTH,
    },
    Service {
        name: "ftp",
        label: "FTP",
        fields: &[
            BackendField::new("endpoint", "Server URL")
                .required()
                .example("ftps://ftp.example.com:21"),
            BackendField::new("user", "User name"),
            BackendField::new("password", "Server password").secret(),
            ROOT,
        ],
    },
    Service {
        name: "gcs",
        label: "Google Cloud Storage",
        fields: &[
            BackendField::new("bucket", "Bucket")
                .aliases(&["google_bucket", "google_bucket_name", "bucket_name"])
                .required(),
            BackendField::new("credential", "Service account key")
                .secret()
                .hint("Base64 of the JSON key. Or use the key file below."),
            BackendField::new("credential_path", "Service account key file")
                .example("/home/me/key.json"),
            ROOT,
        ],
    },
    Service {
        name: "gdrive",
        label: "Google Drive",
        fields: OAUTH,
    },
    Service {
        name: "ipmfs",
        label: "IPFS (MFS)",
        fields: &[
            BackendField::new("endpoint", "API endpoint")
                .example("http://localhost:5001")
                .hint("Empty for the default."),
            ROOT,
        ],
    },
    Service {
        name: "obs",
        label: "Huawei Cloud OBS",
        fields: &[
            BackendField::new("bucket", "Bucket").required(),
            BackendField::new("endpoint", "Endpoint")
                .required()
                .example("https://obs.cn-north-4.myhuaweicloud.com"),
            BackendField::new("access_key_id", "Access key ID"),
            BackendField::new("secret_access_key", "Secret access key").secret(),
            ROOT,
        ],
    },
    Service {
        name: "onedrive",
        label: "OneDrive",
        fields: OAUTH,
    },
    Service {
        name: "oss",
        label: "Alibaba Cloud OSS",
        fields: &[
            BackendField::new("bucket", "Bucket").required(),
            BackendField::new("endpoint", "Endpoint")
                .required()
                .example("https://oss-cn-hangzhou.aliyuncs.com"),
            BackendField::new("access_key_id", "Access key ID"),
            BackendField::new("access_key_secret", "Access key secret").secret(),
            ROOT,
        ],
    },
    Service {
        name: "pcloud",
        label: "pCloud",
        fields: &[
            BackendField::new("endpoint", "API endpoint")
                .required()
                .example("https://api.pcloud.com")
                .hint("Use https://eapi.pcloud.com for EU accounts."),
            BackendField::new("username", "Account email").required(),
            BackendField::new("password", "Account password")
                .required()
                .secret(),
            ROOT,
        ],
    },
    Service {
        name: "s3",
        label: "Amazon S3 (or compatible)",
        fields: &[
            BackendField::new("bucket", "Bucket")
                .aliases(&["aws_bucket", "aws_bucket_name", "bucket_name"])
                .required(),
            BackendField::new("region", "Region")
                .example("us-east-1")
                .hint("AWS needs it. Other providers often accept auto or us-east-1."),
            BackendField::new("endpoint", "Endpoint")
                .example("https://s3.example.com")
                .hint("Leave empty for AWS."),
            BackendField::new("access_key_id", "Access key ID"),
            BackendField::new("secret_access_key", "Secret access key").secret(),
            ROOT,
        ],
    },
    #[cfg(not(windows))]
    Service {
        name: "sftp",
        label: "SFTP",
        fields: &[
            BackendField::new("endpoint", "Server")
                .required()
                .example("ssh://user@host:22"),
            BackendField::new("user", "User name"),
            BackendField::new("key", "Private key file").example("/home/me/.ssh/id_ed25519"),
            ROOT,
        ],
    },
    Service {
        name: "swift",
        label: "OpenStack Swift",
        fields: &[
            BackendField::new("endpoint", "Endpoint").required(),
            BackendField::new("container", "Container").required(),
            BackendField::new("token", "Token").secret(),
            ROOT,
        ],
    },
    Service {
        name: "webdav",
        label: "WebDAV",
        fields: &[
            BackendField::new("endpoint", "Server URL")
                .required()
                .example("https://cloud.example.com/remote.php/dav/files/me"),
            BackendField::new("username", "User name"),
            BackendField::new("password", "Server password").secret(),
            ROOT,
        ],
    },
    Service {
        name: "webhdfs",
        label: "Hadoop WebHDFS",
        fields: &[
            BackendField::new("endpoint", "Endpoint")
                .example("http://127.0.0.1:9870")
                .hint("Empty for the default."),
            BackendField::new("user_name", "User name"),
            BackendField::new("delegation", "Delegation token").secret(),
            ROOT,
        ],
    },
    Service {
        name: "yandex-disk",
        label: "Yandex Disk",
        fields: &[
            BackendField::new("access_token", "Access token")
                .required()
                .secret(),
            ROOT,
        ],
    },
];

fn service(name: &str) -> Option<&'static Service> {
    OPENDAL_SERVICES.iter().find(|service| service.name == name)
}

/// The named fields of an opendal service; empty for a service the picker does not offer.
pub fn opendal_fields(service_name: &str) -> &'static [BackendField] {
    service(service_name).map_or(&[], |service| service.fields)
}

/// The picker name of an opendal service, such as "WebDAV"; `None` for a service the picker does
/// not offer.
pub fn opendal_label(service_name: &str) -> Option<&'static str> {
    service(service_name).map(|service| service.label)
}

/// The docs.rs page of the opendal config struct of a service, which documents every option key.
pub fn opendal_docs_url(service_name: &str) -> String {
    let camel: String = service_name
        .split('-')
        .flat_map(|word| {
            let mut chars = word.chars();
            let first = chars.next().map(|c| c.to_ascii_uppercase());
            first.into_iter().chain(chars)
        })
        .collect();
    format!("https://docs.rs/opendal/0.58/opendal/services/struct.{camel}Config.html")
}
