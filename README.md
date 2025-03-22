# sfv-rs

Personal manifest checksum generation / verification utility.

Occssionally I build manifests of large directory structures that I want to detect file changes in.
I use this to generate a manifest at a point in time for a directory and then run the verification step to detect what files have been touched since I last generated the manifest.

https://github.com/user-attachments/assets/53f4519d-2c61-4c75-a9c5-6c1421596670

## Manifest Generation

```bash
# Simple usage, defaults to sfv.toml output
sfv-rs generate .

# Control over the output manifest file is supported
sfv-rs generate -o mymanifest.toml .

# If a specific checksum algorithm makes the most sense for a directory, I can specify the algorithm
sfv-rs generate -a sha256 .

# Standard GNU formats such as md5sum can also be used
sfv-rs generate -f md5sum .

# Checksum modes are supported, binary mode is always the default
# You will likely run into errors if you attempt to generate text checksums in directories that contain files not using only UTF-8
sfv-rs generate -m text .

# Control over the checksum chunk size is supported
sfv-rs generate -c 1024 .

# Control over the number of checksum workers is supported
sfv-rs generate -x 1 .

# Verbose logging is supported on the root command
# -v or -vv will output all generated manifest checksums
sfv-rs -v generate .
```

## Manifest Verificaiton

```bash
# If I'm currently in a directory with a manifest file, I can verify the manifest
sfv-rs

# If I'm not in a directory with a manifest file, I can target the directory with the manifest
sfv-rs verify -m ./FOLDER_WITH_MANIFEST .

# Control over the checksum chunk size is supported
sfv-rs verify -c 1024 .

# Control over the number of checksum workers is supported
sfv-rs verify -x 1 .

# Verbose logging is supported on the root command
# No verbose flag will always output verification failures
# -v will output warnings (such as missing files)
# -vv will output all verification results (including all successful verifications)
sfv-rs -vv
```
