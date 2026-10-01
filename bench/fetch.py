"""Download the HN dataset and the Typesense and Meilisearch binaries into bench/.cache, verifying checksums."""
import argparse, hashlib, io, json, os, platform, sys, tarfile
import requests
from common import BIN, DATA

DATASET = "https://milli-benchmarks.fra1.digitaloceanspaces.com/bench/datasets/hackernews/hackernews-{}.ndjson"
DATASET_SHA256 = {
    "100_000": "60ecd23485d560edbd90d9ca31f0e6dba1455422f2a44e402600fbb5f7f1b213",
    "200_000": "785b0271fdb47cba574fab617d5d332276b835c05dd86e4a95251cf7892a1685",
    "300_000": "de73c7154652eddfaf69cdc3b2f824d5c452f095f40a20a1c97bb1b5c4d80ab2",
    "400_000": "c1b00a24689110f366447e434c201c086d6f456d54ed1c4995894102794d8fe7",
    "500_000": "ae98f9dbef8193d750e3e2dbb6a91648941a1edca5f6e82c143e7996f4840083",
    "600_000": "b495fdc72c4a944801f786400f22076ab99186bee9699f67cbab2f21f5b74dbe",
    "700_000": "4b2c63974f3dabaa4954e3d4598b48324d03c522321ac05b0d583f36cb78a28b",
    "800_000": "cb7b6afe0e6caa1be111be256821bc63b0771b2a0e1fad95af7aaeeffd7ba546",
    "900_000": "e1154ddcd398f1c867758a93db5bcb21a07b9e55530c188a2917fdef332d3ba9",
    "1_000_000": "27e25efd0b68b159b8b21350d9af76938710cb29ce0393fa71b41c4f3c630ffe",
}

TYPESENSE_VERSION = "30.2"
TYPESENSE_URL = "https://dl.typesense.org/releases/{v}/typesense-server-{v}-{p}.tar.gz"
TYPESENSE_SHA256 = {
    "darwin-arm64": "7d8d6d0c33930ad20ea23dd184250547b16615944be891b2078e8a075152fa7e",
    "darwin-amd64": "6aa4d2d85838e03fdde9dcef4bf1584a46fc1840a215e5cbed288b63365eae75",
    "linux-amd64": "cd791605e7c6cc7be457794f534cd4b9f9d361781adb10e40020efc22840ebc3",
    "linux-arm64": "a9b9d6e2927c863900306290e8532c17f66bdff34ef2ad043b24ed4ccec8a6bc",
}
MEILI_VERSION = "1.54.2"
MEILI_URL = "https://github.com/meilisearch/meilisearch/releases/download/v{v}/meilisearch-{p}"
MEILI_ASSET = {"darwin-arm64": "macos-apple-silicon", "darwin-amd64": "macos-amd64",
               "linux-amd64": "linux-amd64", "linux-arm64": "linux-aarch64"}
MEILI_SHA256 = {
    "darwin-arm64": "56d8c9172ba071561ad5575d47686c338915b566bac9e153a1f2300c7cdcee32",
    "darwin-amd64": "316ea5eb916b49bd0781a9c4e0a04dfa2872ebd61eb3b05a94ac7a94a77e3486",
    "linux-amd64": "fb881e0b13aeac2c40b423a69c501b479b4abb1032d7f057efbe1fa32a99225b",
    "linux-arm64": "757bf3c18bc629234ca724b1b2b845eb937a84b9a717c3599bc7a56d47941781",
}


def host():
    system = {"Darwin": "darwin", "Linux": "linux"}.get(platform.system())
    arch = {"arm64": "arm64", "aarch64": "arm64", "x86_64": "amd64", "AMD64": "amd64"}.get(platform.machine())
    if not system or not arch:
        sys.exit(f"unsupported platform {platform.system()} {platform.machine()}")
    return f"{system}-{arch}"


def download(url, sha256):
    """Fetch url into memory and check its SHA-256."""
    with requests.get(url, stream=True, timeout=120) as r:
        r.raise_for_status()
        buf = io.BytesIO()
        h = hashlib.sha256()
        for chunk in r.iter_content(1 << 20):
            h.update(chunk)
            buf.write(chunk)
    if h.hexdigest() != sha256:
        sys.exit(f"checksum mismatch for {url}: {h.hexdigest()}")
    return buf.getvalue()


def keep(line):
    """Live stories with a title and a score, reduced to id, title and score."""
    if not line.strip():
        return None
    d = json.loads(line)
    title = (d.get("title") or "").strip()
    if d.get("type") == "story" and title and "score" in d and not d.get("dead") and not d.get("deleted"):
        return json.dumps({"id": d["id"], "title": title, "score": d["score"]})
    return None


def fetch_dataset():
    if DATA.exists():
        return
    DATA.parent.mkdir(parents=True, exist_ok=True)
    tmp = DATA.with_suffix(".tmp")
    n = 0
    with tmp.open("w") as out:
        for part, sha in DATASET_SHA256.items():
            for line in download(DATASET.format(part), sha).splitlines():
                if row := keep(line):
                    out.write(row + "\n")
                    n += 1
            print(f"hackernews-{part}: ok, {n} stories so far", file=sys.stderr)
    tmp.rename(DATA)


def fetch_binaries():
    p = host()
    BIN.mkdir(parents=True, exist_ok=True)
    ts = BIN / "typesense-server"
    if not ts.exists():
        data = download(TYPESENSE_URL.format(v=TYPESENSE_VERSION, p=p), TYPESENSE_SHA256[p])
        with tarfile.open(fileobj=io.BytesIO(data)) as tar:
            ts.write_bytes(tar.extractfile("typesense-server").read())
        ts.chmod(0o755)
    ms = BIN / "meilisearch"
    if not ms.exists():
        ms.write_bytes(download(MEILI_URL.format(v=MEILI_VERSION, p=MEILI_ASSET[p]), MEILI_SHA256[p]))
        ms.chmod(0o755)


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("what", nargs="?", choices=["all", "data", "bin", "wiki"], default="all")
    a = ap.parse_args()
    if a.what == "wiki":
        import wiki
        wiki.main()
    if a.what in ("all", "data"):
        fetch_dataset()
    if a.what in ("all", "bin"):
        fetch_binaries()
    print("ok", os.fspath(DATA.parent.parent), file=sys.stderr)
