"""Publish Android update metadata from the APK's actual Gradle output."""

import argparse
import hashlib
import json
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--metadata", type=Path, required=True)
    parser.add_argument("--apk", type=Path, required=True)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    metadata = json.loads(args.metadata.read_text())
    assert metadata["applicationId"] == "com.akily.p2pshare", "Unexpected APK application ID"
    assert len(metadata["elements"]) == 1, "Expected one universal APK"
    build = metadata["elements"][0]
    assert build["versionName"] == args.version, "APK version does not match release tag"
    assert args.apk.name == f"p2p-share-android-v{args.version}.apk", "Unexpected APK filename"
    with args.apk.open("rb") as apk:
        digest = hashlib.file_digest(apk, "sha256").hexdigest()
    args.output.write_text(json.dumps({
        "versionCode": build["versionCode"],
        "versionName": build["versionName"],
        "url": f"https://github.com/{args.repository}/releases/download/v{args.version}/{args.apk.name}",
        "size": args.apk.stat().st_size,
        "sha256": digest,
    }, indent=2) + "\n")


if __name__ == "__main__":
    main()
