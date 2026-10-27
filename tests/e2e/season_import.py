"""Past-season import: archive a season on one instance, download its bundle,
import it on a fresh instance (web upload and CLI), and see it on Past
Seasons there; duplicates and foreign tarballs are refused."""
import io
import os
import re
import subprocess
import sys
import tarfile

from lib import Server, check, get, post, summary, wait_for

YAMLS = [b"name: Ann\ngame: ChecksFinder\nChecksFinder: {}\n", b"name: Bob\ngame: ChecksFinder\nChecksFinder: {}\n"]

a = Server("export", port=18188, ap_ports="39160-39162")
a.start()
try:
    root = a.setup_sysadmin()
    post(root, "/admin/seasons", data=dict(name="Spring Cup"))
    sid = int(re.search(r"/s/(\d+)/admin", get(root, "/admin/seasons").text).group(1))
    post(root, f"/s/{sid}/admin/signups", data=dict(open="1"))
    for y in YAMLS:
        post(root, f"/s/{sid}/signup", files={"yaml": ("y.yaml", y)})
    post(root, f"/s/{sid}/admin/generate")
    wait_for(lambda: "Generated seed" in get(root, f"/s/{sid}/admin").text, 300, 3)
    archived = wait_for(lambda: "isn't connected" not in post(root, f"/s/{sid}/admin/archive-season", data={"name": ""}).text, 120, 3)
    check("season archived", archived and "summary" in get(root, "/admin/seasons").text)
    seasons = get(root, "/admin/seasons").text
    check("archived season offers its bundle", f"/s/{sid}/admin/bundle" in seasons)
    r = get(root, f"/s/{sid}/admin/bundle")
    check("bundle downloads", r.ok and r.content[:2] == b"\x1f\x8b", (r.status_code, r.headers.get("content-type")))
    bundle = r.content
    with tarfile.open(fileobj=io.BytesIO(bundle)) as tf:
        names = tf.getnames()
    check("bundle carries bundle.json, summary.json and the replay artifacts",
          all(f"./{n}" in names for n in ["bundle.json", "summary.json", "multidata.zip", "season.txt"]), names)
    anon = a.session()
    check("anonymous can't download it", get(anon, f"/s/{sid}/admin/bundle").status_code in (302, 303, 401, 403)
          or "Sign in" in get(anon, f"/s/{sid}/admin/bundle").text)
finally:
    a.stop()

path = os.path.join(a.data, "..", "spring.tar.gz")
with open(path, "wb") as f:
    f.write(bundle)

b = Server("import", port=18189, ap_ports="39163-39165")
b.start()
try:
    root = b.setup_sysadmin()
    check("import form on the seasons console", "/admin/seasons/import" in get(root, "/admin/seasons").text)
    r = post(root, "/admin/seasons/import", files={"bundle": ("spring.tar.gz", bundle)})
    m = re.search(r"/s/(\d+)/season", r.url)
    check("web import lands on the season's summary", m and "Spring Cup" in r.text, r.url)
    page = get(root, "/").text
    check("it's on Past Seasons", "Spring Cup" in page)
    if m:
        s = get(root, f"/s/{m.group(1)}/season").text
        check("summary shows its games", "ChecksFinder" in s and "Ann" in s and "Bob" in s)
        check("and it offers the bundle again", f"/s/{m.group(1)}/admin/bundle" in get(root, "/admin/seasons").text)
    r = post(root, "/admin/seasons/import", files={"bundle": ("spring.tar.gz", bundle)})
    check("importing it twice is refused", "imported already" in r.text)

    junk = io.BytesIO()
    with tarfile.open(fileobj=junk, mode="w:gz") as tf:
        data = b"name: old\n"
        ti = tarfile.TarInfo("./season.txt")
        ti.size = len(data)
        tf.addfile(ti, io.BytesIO(data))
    r = post(root, "/admin/seasons/import", files={"bundle": ("old.tar.gz", junk.getvalue())})
    check("a bundle without bundle.json is refused plainly", "no bundle.json" in r.text, re.findall(r"Import failed[^<]*", r.text))
    bomb = io.BytesIO()
    with tarfile.open(fileobj=bomb, mode="w:gz") as tf:
        big = 200 << 20  # zeros: compresses to ~200 KiB
        ti = tarfile.TarInfo("./bundle.json")
        ti.size = big

        class Zeros(io.RawIOBase):
            left = big

            def readable(self):
                return True

            def readinto(self, b):
                n = min(len(b), self.left)
                b[:n] = bytes(n)
                self.left -= n
                return n
        tf.addfile(ti, io.BufferedReader(Zeros()))
    r = post(root, "/admin/seasons/import", files={"bundle": ("bomb.tar.gz", bomb.getvalue())})
    check("an oversized member is refused, not buffered", "larger than 16 MiB" in r.text, re.findall(r"Import failed[^<]*", r.text))
    user = b.invite_user(root, "pleb")
    r = post(user, "/admin/seasons/import", files={"bundle": ("spring.tar.gz", bundle)})
    check("players can't import", r.status_code in (401, 403) or "only" in r.text.lower())
finally:
    b.stop()

# The CLI, on a third instance's data dir (server not running).
c = Server("import-cli", port=18190, ap_ports="39166-39168")
c.start()
c.stop()
r = c.cli("season", "import", path)
check("CLI import", r.returncode == 0 and "imported as season" in r.stdout, r.stdout + r.stderr)
r = c.cli("season", "import", path)
check("CLI refuses a duplicate", r.returncode == 1 and "imported already" in r.stderr, r.stderr)

sys.exit(summary())
