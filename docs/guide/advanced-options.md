# Advanced options

Reference for the **Advanced options** key and value list of the repository form. Back to the
[guide index](README.md).

Checked against opendal 0.58.2 and rustic_backend 0.7.0, the versions in `Cargo.lock`. A newer
build may add, rename or drop keys. Maintainers re-check this page when either is bumped
([Releasing](releasing.md)).

## How the key and value pairs work

Each row is one key and one value, both typed as text. salvajocs passes the rows to rustic, which
passes them to the storage library (OpenDAL) or to its own backend code. Nothing is converted on
the way, so the rules below are those of the code that reads the value.

- Rows with an empty key are ignored. The key is trimmed. The value is **not**: a space before or
  after it is part of the value, so `true ` is not `true`.
- Tick **secret** on a row that holds a key or token. It is then kept in the keyring, not in the
  config file.
- A key with no meaning for the backend gets a warning under the row: "Not an option of this
  backend". It is still saved and passed on, and it does nothing.
- A key set in a form field and again in Advanced options is refused, and so is a key set twice.
  For the services, `Region` and `region` are the same key, and so are a key of a form field and
  its alias names (listed per service). The form refuses all of these. It does not compare the
  alias names of keys that have no form field.

### Service keys (OpenDAL)

For the services in the picker, such as WebDAV or S3, the keys are those of the service in the
sections below. They are snake_case: `disable_create_dir`.

| Value type | What is accepted | Wrong value |
| --- | --- | --- |
| Text | Anything, as typed. An empty value means "not set" for an optional key. | |
| Yes or no | `true` or `false`, in lowercase. | `1`, `0`, `yes` and an empty value are errors. |
| Number | Digits, with a leading `-` where negative makes sense. No spaces. | An error. |

- Key names are matched in any letter case: `Disable_Create_Dir` is `disable_create_dir`.
- A key the service does not have is ignored by opendal. The form shows a warning under the row.
  A typo does nothing, so check the spelling against the tables below.
- Keys marked deprecated are still read, and still must be valid. They have no effect in this
  version, except `allow_anonymous`, which still works as `skip_signature`.
- No service has a list value in this version. The S3 key `assume_role_session_tags` is a map and
  cannot be set from a text value.

Source: `serde_util.rs` in `opendal-core` (`deserialize_bool`, `deserialize_option`, and the
number parsers).

### Keys that rustic reads

rustic reads a few keys itself, before the backend sees them. They are kebab-case or plain
lowercase, and they must be typed in lowercase: unlike the service keys, `Retry` is ignored.

| Backend | Key | Value | What it does |
| --- | --- | --- | --- |
| Services | `retry` | `default`, `false` or a whole number. Default `5`. | How many times a failed request is retried. `false` means 0. |
| Services | `connections` | A whole number, at least 1. Default: no limit. | The most requests at the same time. |
| Services | `throttle` | `bandwidth,burst`, each a size such as `10MiB`. Both are needed, neither can be `0`, and each must be under 4 GiB. | Limits the upload speed, in bytes per second. Reads are not limited. A single write larger than the burst fails, and a pack is written in one call, so the burst must be at least the largest pack: tens of MiB. For example `10MiB,256MiB`. |
| REST server, rclone remote | `retry` | As above. | As above, for the REST requests. |
| REST server, rclone remote | `timeout` | A duration: `30s`, `10m`, `2h 30m` or `PT30S`. Default `10m`. | How long one request may take. A negative value sets a timeout of zero, so every request fails. |
| REST server, rclone remote | `cacert` | A path to a PEM file. | Adds a certificate authority to trust. |
| REST server, rclone remote | `tls-client-cert` | A path to a PEM file with the certificate and the private key. | The client certificate to present. |

An invalid `retry`, `connections`, `throttle` or `timeout` value, or a `cacert` or
`tls-client-cert` file that cannot be read, is an error. An unknown key is ignored.

The service tables below do not repeat `retry`, `connections` and `throttle`. They are not
service keys: rustic reads them and passes the rows on, and the service ignores them.

### What you see on a wrong value

Press **Continue**. salvajocs opens the repository, and the error shows in the form. It names the
key and the value. For example, `disable_create_dir` set to `1` on WebDAV:

> Creating Operator from path `webdav` failed. Please check the given schema and options. ...
> parse config 'disable_create_dir' with value '1' failed for "invalid bool value"

A bad `retry` shows "Parsing retry value `abc` failed, the value must be a valid integer.".

## Local folder

Keys of the local backend. Any other key is ignored with a warning in the log.

| Key | Type | Default | What it does |
| --- | --- | --- | --- |
| `post-create-command` | Text | Not set | A command run after each file is written. `%file`, `%type` and `%id` are replaced by the path, the kind and the id of the file. |
| `post-delete-command` | Text | Not set | The same, after each file is removed. |

A command that fails is only logged. It does not fail the backup.

## REST server

The keys are `retry`, `timeout`, `cacert` and `tls-client-cert` from the table
[above](#keys-that-rustic-reads).

## rclone remote

The REST keys above apply to the connection to the `rclone` program. These keys are for the
program:

| Key | Type | Default | What it does |
| --- | --- | --- | --- |
| `rclone-command` | Text | `rclone serve restic --addr localhost:0` | The command that starts the server. The location is added at the end. |
| `use-password` | `true` or `false` | `true` | Whether a random user and password are set on the local server. |
| `rest-url` | Text | Read from the output of rclone | Use this URL, and do not read it from the output of rclone. With `use-password` it must start with `http://`. |

## Alibaba Cloud OSS

Service `oss`. The form has `bucket`, `endpoint`, `access_key_id`, `access_key_secret` and
`root`. Keys left unset are read from the `ALIBABA_CLOUD_*` environment variables.

| Key | Type | Default | What it does |
| --- | --- | --- | --- |
| **`addressing_style`** | `virtual`, `cname` or `path` | `virtual` | How the bucket is put in the URL. `path` puts the bucket in the path, `cname` uses your own domain. |
| `presign_endpoint` | Text | Not set | Endpoint for presigned URLs. |
| `presign_addressing_style` | Text | As `addressing_style` | Style for presigned URLs. `virtual` when only `presign_endpoint` is set. |
| `server_side_encryption` | Text | Not set | Server side encryption method. |
| `server_side_encryption_key_id` | Text | Not set | Key id for the encryption. |
| `skip_signature` | Yes or no | `false` | Send requests without credentials. |
| `security_token` | Text, secret | Not set | A temporary token that goes with temporary keys. |
| `role_arn`, `role_session_name`, `external_id` | Text | Not set | Assume a role with the keys given. |
| `oidc_provider_arn`, `oidc_token_file` | Text | Not set | Sign in with an OIDC token. |
| `sts_endpoint` | Text | Not set | The STS endpoint for the role. |

Deprecated: `enable_versioning`, `batch_max_operations`, `delete_max_size`, and `allow_anonymous`,
which still works as `skip_signature`.

## Amazon S3 (or compatible)

Service `s3`. The form has `bucket`, `region`, `endpoint`, `access_key_id`, `secret_access_key`
and `root`. When keys are not set, S3 reads the AWS environment variables and `~/.aws` files, then
the EC2 metadata service.

| Key | Type | Default | What it does |
| --- | --- | --- | --- |
| **`enable_virtual_host_style`** | Yes or no | `false` | Put the bucket in the host name, `bucket.host`, instead of the path. Some providers need it. |
| **`disable_ec2_metadata`** | Yes or no | `false` | Do not ask the EC2 metadata service for credentials. Useful on a provider that is not AWS. |
| **`disable_config_load`** | Yes or no | `false` | Do not read AWS environment variables and `~/.aws` files. |
| `profile` | Text | Not set | The AWS profile to read from `~/.aws`. |
| `session_token` | Text, secret | Not set | Token of temporary credentials. |
| `role_arn`, `external_id`, `role_session_name` | Text | Not set | Assume a role, with the keys already known as the source. |
| `assume_role_duration_seconds` | Number | Not set | How long the assumed role lasts. |
| `skip_signature` | Yes or no | `false` | Send requests without credentials. |
| `default_storage_class` | Text | Not set | One of `STANDARD`, `STANDARD_IA`, `ONEZONE_IA`, `INTELLIGENT_TIERING`, `GLACIER`, `GLACIER_IR`, `DEEP_ARCHIVE`, `EXPRESS_ONEZONE`, `OUTPOSTS`, `REDUCED_REDUNDANCY`. Compatible services support fewer. |
| `checksum_algorithm` | `crc32c` or `md5` | Not set | Checksum sent with each write. Needed for buckets with Object Lock. |
| `server_side_encryption` | `AES256` or `aws:kms` | Not set | Server side encryption. |
| `server_side_encryption_aws_kms_key_id` | Text | Not set | The KMS key for `aws:kms`. AWS picks its managed key when empty. |
| `server_side_encryption_customer_algorithm` | `AES256` | Not set | Encryption with a key you provide. |
| `server_side_encryption_customer_key` | Base64 text, secret | Not set | The key for that. |
| `server_side_encryption_customer_key_md5` | Text | Not set | The MD5 of that key. |
| `disable_list_objects_v2` | Yes or no | `false` | Use the old list call, for services without V2. |
| `enable_request_payer` | Yes or no | `false` | Agree to pay for the requests, on a bucket set to "requester pays". |
| `default_acl` | Text | Not set | The ACL of new objects. Some services such as MinIO do not support it. |

Aliases: `bucket` is also `aws_bucket`, `aws_bucket_name`, `bucket_name`. `endpoint` is also
`aws_endpoint`, `aws_endpoint_url`, `endpoint_url`. `region` is also `aws_region`. `profile` is
also `aws_profile`. `access_key_id` is also `aws_access_key_id`. `secret_access_key` is also
`aws_secret_access_key`. `session_token` is also `aws_session_token`, `aws_token`, `token`.
`server_side_encryption` is also `aws_server_side_encryption`.
`server_side_encryption_aws_kms_key_id` is also `aws_sse_kms_key_id`.
`server_side_encryption_customer_key` is also `aws_sse_customer_key_base64`.
`enable_virtual_host_style` is also `aws_virtual_hosted_style_request`,
`virtual_hosted_style_request`. `checksum_algorithm` is also `aws_checksum_algorithm`.
`enable_request_payer` is also `aws_request_payer`, `request_payer`.

Cannot be set: `assume_role_session_tags`, a map.
Deprecated: `enable_versioning`, `batch_max_operations`, `delete_max_size`,
`disable_stat_with_override`, `disable_write_with_if_match`, `enable_write_with_append`, and
`allow_anonymous`, which still works as `skip_signature`.

## Azure Blob Storage

Service `azblob`. The form has `container`, `endpoint`, `account_name`, `account_key`,
`sas_token` and `root`.

| Key | Type | Default | What it does |
| --- | --- | --- | --- |
| `encryption_key` | Text, secret | Not set | A customer-provided encryption key. |
| `encryption_key_sha256` | Text | Not set | The SHA-256 of that key. |
| `encryption_algorithm` | Text | Not set | The algorithm of that key. |
| `skip_signature` | Yes or no | `false` | Send requests without credentials. |

Aliases: `container` is also `azure_container_name`, `container_name`. `endpoint` is also
`azure_storage_endpoint`, `azure_endpoint`. `account_name` is also `azure_storage_account_name`.
`account_key` is also `azure_storage_account_key`, `azure_storage_access_key`,
`azure_storage_master_key`, `access_key`, `master_key`. `sas_token` is also
`azure_storage_sas_key`, `azure_storage_sas_token`, `sas_key`.
Deprecated: `batch_max_operations`.

## Azure Data Lake Storage

Service `azdls`. The form has `filesystem`, `endpoint`, `account_name`, `account_key` and
`root`. The account key is one way to sign in. The two below are the others.

| Key | Type | Default | What it does |
| --- | --- | --- | --- |
| **`sas_token`** | Text, secret | Not set | Sign in with a shared access signature. |
| **`client_id`**, **`tenant_id`**, **`client_secret`** | Text, `client_secret` is secret | Not set | Sign in with a service principal. All three are needed. |
| `authority_host` | Text | `https://login.microsoftonline.com` | The sign-in host for the service principal. |
| `enable_hns` | Yes or no | `false` | The storage account has hierarchical namespace. Lets large folders be deleted in pages. |

## Azure Files

Service `azfile`. The form has `share_name`, `endpoint`, `account_name`, `account_key` and
`root`.

| Key | Type | Default | What it does |
| --- | --- | --- | --- |
| **`sas_token`** | Text, secret | Not set | Sign in with a shared access signature instead of the account key. |

## Backblaze B2

Service `b2`. The form has every key of the service: `bucket`, `bucket_id`, `application_key_id`,
`application_key` and `root`. There are no more.

## Dropbox

Service `dropbox`. The form has every key of the service: `access_token`, `refresh_token`,
`client_id`, `client_secret` and `root`. There are no more.

## FTP

Service `ftp`. The form has every key of the service: `endpoint`, `user`, `password` and `root`.
There are no more.

## Google Cloud Storage

Service `gcs`. The form has `bucket`, `credential`, `credential_path` and `root`.

| Key | Type | Default | What it does |
| --- | --- | --- | --- |
| **`token`** | Text, secret | Not set | A ready OAuth2 token. It wins over `credential` and `credential_path`. |
| **`disable_vm_metadata`** | Yes or no | `false` | Do not ask the Google Cloud metadata server for credentials. |
| `disable_config_load` | Yes or no | `false` | Do not read credentials from the environment. |
| `skip_signature` | Yes or no | `false` | Send requests without credentials. |
| `endpoint` | Text | `https://storage.googleapis.com` | The service URL. |
| `scope` | Text | `https://www.googleapis.com/auth/devstorage.read_write` | The OAuth2 scope. Read only is `https://www.googleapis.com/auth/devstorage.read_only`. |
| `service_account` | Text | `default` | The service account to use when credentials come from the metadata server. |
| `predefined_acl` | Text | Not set | One of `authenticatedRead`, `bucketOwnerFullControl`, `bucketOwnerRead`, `private`, `projectPrivate`, `publicRead`. |
| `default_storage_class` | Text | Not set | The storage class of new objects. |

Aliases: `credential` is also `google_service_account_key`, `service_account_key`.
`credential_path` is also `google_application_credentials`. `skip_signature` is also
`google_skip_signature`. `bucket` is also `google_bucket`, `google_bucket_name`, `bucket_name`.
`service_account` is also `google_service_account`, `google_service_account_path`,
`service_account_path`.
Deprecated: `allow_anonymous`, which still works as `skip_signature`.

## Google Drive

Service `gdrive`. The form has every key of the service: `access_token`, `refresh_token`,
`client_id`, `client_secret` and `root`. There are no more.

## Hadoop WebHDFS

Service `webhdfs`. The form has `endpoint`, `user_name`, `delegation` and `root`.

| Key | Type | Default | What it does |
| --- | --- | --- | --- |
| `atomic_write_dir` | Text | Not set | A folder for temporary files, such as `.opendal_tmp/`. Needed for writes in several parts. |
| `disable_list_batch` | Yes or no | `false` | Do not list folders in batches. |

## Huawei Cloud OBS

Service `obs`. The form has every key of the service: `bucket`, `endpoint`, `access_key_id`,
`secret_access_key` and `root`. There are no more. Deprecated: `enable_versioning`.

## IPFS (MFS)

Service `ipmfs`. The form has every key of the service: `endpoint` and `root`. There are no more.

## OneDrive

Service `onedrive`. The form has every key of the service: `access_token`, `refresh_token`,
`client_id`, `client_secret` and `root`. There are no more. Deprecated: `enable_versioning`.

## OpenStack Swift

Service `swift`. The form has `endpoint`, `container`, `token` and `root`.

| Key | Type | Default | What it does |
| --- | --- | --- | --- |
| `temp_url_key` | Text, secret | Not set | The key for presigned URLs. salvajocs does not use presigned URLs. |
| `temp_url_hash_algorithm` | `sha1`, `sha256` or `sha512` | `sha256` | The hash for presigned URLs. |

## pCloud

Service `pcloud`. The form has every key of the service: `endpoint`, `username`, `password` and
`root`. There are no more.

## SFTP

Service `sftp`. Not in Windows builds. The form has `endpoint`, `user`, `key` and `root`.

| Key | Type | Default | What it does |
| --- | --- | --- | --- |
| **`known_hosts_strategy`** | `Strict`, `Accept` or `Add`, in any letter case | `Strict` | What to do with a server that is not in `known_hosts`. `Strict` refuses it. `Accept` trusts it without saving it. `Add` trusts it and saves it. Any other value is an error. |

Deprecated: `enable_copy`.

## Tencent Cloud COS

Service `cos`. The form has `bucket`, `endpoint`, `secret_id`, `secret_key` and `root`.

| Key | Type | Default | What it does |
| --- | --- | --- | --- |
| `security_token` | Text, secret | Not set | A temporary token that goes with temporary keys. |
| `disable_config_load` | Yes or no | `false` | Do not read credentials from the environment. |

Deprecated: `enable_versioning`.

## WebDAV

Service `webdav`. The form has `endpoint`, `username`, `password` and `root`.

| Key | Type | Default | What it does |
| --- | --- | --- | --- |
| **`token`** | Text, secret | Not set | Sign in with a bearer token instead of a user name and password. |
| **`disable_create_dir`** | Yes or no | `false` | Do not create parent folders before writing. For servers without PROPFIND support. |
| `enable_conditional_read` | Yes or no | `true` | Send `If-Match` and similar headers on reads. Set `false` for servers that ignore them, such as nginx-dav. |
| `user_metadata_prefix` | Text | `opendal` | The XML prefix of the custom properties. |
| `user_metadata_uri` | Text | `https://opendal.apache.org/ns` | The XML namespace of the custom properties. |

Deprecated: `disable_copy`, `enable_user_metadata`. `disable_copy` takes `true` or `false`, but it
does nothing: copy is always on.

## Yandex Disk

Service `yandex-disk`. The form has every key of the service: `access_token` and `root`. There
are no more.
