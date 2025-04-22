# artsum

Personal manifest checksum generation / verification utility.

Occssionally I build manifests of large directory structures that I want to detect file changes in.
I use this to generate a manifest at a point in time for a directory and then run the verification step to detect what files have been touched since I last generated the manifest.

https://github.com/user-attachments/assets/53f4519d-2c61-4c75-a9c5-6c1421596670

## Manifest Generation

```bash
# Simple usage, defaults to sfv.toml output
artsum generate .

# Control over the output manifest file is supported
artsum generate -o mymanifest.toml .

# If a specific checksum algorithm makes the most sense for a directory, I can specify the algorithm
artsum generate -a sha256 .

# Standard GNU formats such as md5sum can also be used
artsum generate -f md5sum .

# Checksum modes are supported, binary mode is always the default
# You will likely run into errors if you attempt to generate text checksums in directories that contain files not using only UTF-8
artsum generate -m text .

# Control over the checksum chunk size is supported
artsum generate -c 1024 .

# Control over the number of checksum workers is supported
artsum generate -x 1 .

# Verbose logging is supported on the root command
# -v or -vv will output all generated manifest checksums
artsum -v generate .
```

## Manifest Verification

```bash
# If I'm currently in a directory with a manifest file, I can verify the manifest
artsum

# If I'm not in a directory with a manifest file, I can target the directory with the manifest
artsum verify -m [MANIFEST_FILEPATH] .

# Control over the checksum chunk size is supported
artsum verify -c 1024 .

# Control over the number of checksum workers is supported
artsum verify -x 1 .

# Verbose logging is supported on the root command
# No verbose flag will always output verification failures
# -v will output warnings (such as missing files)
# -vv will output all verification results (including all successful verifications)
artsum -vv
```

## Refresh a Manifest

```bash
# If I'm currently in a directory with a manifest file, I can refresh the manifest's checksums
artsum refresh .

# If I'm not in a directory with a manifest file, I can target the directory with the manifest
artsum refresh -m [MANIFEST_FILEPATH] .

# Control over the checksum chunk size is supported
artsum refresh -c 1024 .

# Control over the number of checksum workers is supported
artsum refresh -x 1 .

# Verbose logging is supported on the root command
# No verbose flag will always output updated or removed artifacts
# -v will output unchanged files
artsum -v refresh .
```
