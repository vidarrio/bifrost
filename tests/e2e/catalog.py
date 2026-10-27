"""Apworld catalog + approval: manual approval holds uploads and their
signups, approve/reject, catalog picks and catalog-matching uploads, auto mode."""
import json
import re
import sys
import time

from lib import Server, check, fixture, get, post, summary, wait_for

srv = Server("catalog").start()
try:
    root = srv.setup_sysadmin()
    alice = srv.invite_user(root, "alice")
    bob = srv.invite_user(root, "bob")

    r = post(root, "/admin/seasons", data=dict(name="Spring"))
    sid = int(re.search(r'/s/(\d+)/admin', get(root, "/admin/seasons").text).group(1))
    S = f"/s/{sid}"
    post(root, f"{S}/admin/signups", data=dict(open="1"))

    # --- permissions
    check("anonymous can't see the catalog", "Sysadmins" in get(srv.session(), "/admin/catalog").text
          or get(srv.session(), "/admin/catalog", allow_redirects=False).status_code in (302, 303, 401, 403))
    r = get(alice, "/admin/catalog", allow_redirects=False)
    check("a plain user can't see the catalog", r.status_code in (302, 303, 403) or "Sysadmins" in r.text, r.status_code)
    r = get(root, "/admin/catalog")
    check("sysadmin sees the catalog page, manual by default",
          r.ok and 'value="manual" checked' in r.text and "The catalog is empty" in r.text)

    def upload(s, name, data=None):
        files = {"apworld": (name, data if data is not None else fixture(name))}
        return post(s, f"{S}/signup/apworld", files=files).json()

    # --- manual: upload waits
    j = upload(alice, "foo.apworld")
    check("manual mode: upload is pending", j.get("ok") and j.get("pending") is True and j.get("world") == "foo.apworld", j)
    j = upload(bob, "foo.apworld")
    check("same file again: still pending (no error)", j.get("ok") and j.get("pending"), j)
    j = upload(bob, "foo.apworld", fixture("foo-v2.apworld"))
    check("different content under a pending name is refused", not j.get("ok") and "waiting for approval" in j.get("error", ""), j)
    r = post(alice, f"{S}/signup/check", files={"yaml": ("foo.yaml", fixture("foo.yaml"))}).json()
    check("pending world isn't loaded by the checker", not r.get("ok"), r)

    r = post(alice, f"{S}/signup", files={"yaml": ("foo.yaml", fixture("foo.yaml"))},
             data={"is_custom": "1", "pending_world": "foo.apworld"})
    check("held signup accepted", "held until a moderator approves foo.apworld" in r.text, r.text[-400:])
    r = post(bob, f"{S}/signup", files={"yaml": ("x.yaml", b"game: Foo Finder\n")},
             data={"is_custom": "1", "pending_world": "foo.apworld"})
    check("held signup needs a top-level name", "Couldn't find a top-level name" in r.text)
    r = post(bob, f"{S}/signup", files={"yaml": ("x.yaml", fixture("foo.yaml"))},
             data={"is_custom": "1", "pending_world": "nope.apworld"})
    check("held signup for an unknown pending world is refused", "isn't waiting for approval" in r.text)

    page = get(alice, f"{S}/signup").text
    check("roster shows the held signup", "waiting for foo.apworld to be approved" in page)
    adm = get(root, f"{S}/admin").text
    check("admin page lists the request", "Apworlds waiting for approval" in adm and "Foo Finder" in adm)
    check("admin signup list marks it held", "held for foo.apworld" in adm)

    post(root, f"{S}/admin/generate")
    msg = wait_for(lambda: (m := re.search(r"Failed[^<]*", get(root, f"{S}/admin").text)) and m.group(0), 30)
    check("generate refuses while signups are held", msg and "held" in msg, msg)

    r = post(alice, f"{S}/admin/worlds/approve", data={"name": "foo.apworld"})
    check("a player can't approve", "Admins only" in r.text or r.status_code in (403, 302, 303), r.status_code)

    post(root, f"{S}/admin/worlds/approve", data={"name": "foo.apworld"})
    adm = get(root, f"{S}/admin").text
    check("approved world is installed with provenance", "approved by root" in adm and "Apworlds waiting" not in adm)
    released = wait_for(lambda: "waiting for foo.apworld" not in get(alice, f"{S}/signup").text, 120, 2)
    page = get(alice, f"{S}/signup").text
    check("held signup released after its check", released and "Pfoo" in page and "held" not in page.split("Signed up")[-1], page[-600:])
    r = post(alice, f"{S}/signup/check", files={"yaml": ("foo.yaml", fixture("foo.yaml"))}).json()
    check("approved world now checks", r.get("ok"), r)

    j = upload(bob, "foo.apworld", fixture("foo-v2.apworld"))
    check("different content under an installed name is refused", not j.get("ok") and "already installed" in j.get("error", ""), j)

    # --- reject
    j = upload(bob, "baz.apworld")
    post(bob, f"{S}/signup", files={"yaml": ("baz.yaml", fixture("baz.yaml"))},
         data={"is_custom": "1", "pending_world": "baz.apworld"})
    post(root, f"{S}/admin/worlds/reject", data={"name": "baz.apworld"})
    page = get(bob, f"{S}/signup").text
    check("rejected: held signup says so", "baz.apworld was rejected by root" in page, page[-500:])
    adm = get(root, f"{S}/admin").text
    check("rejected world isn't installed", "baz.apworld" not in adm.split("Installed apworlds")[1].split("Clear all apworlds")[0])
    # remove bob's dead signup
    sub_id = re.search(r'name="id" value="(\d+)"><button class="ghost" type="submit">Remove', get(bob, f"{S}/signup").text)
    if sub_id:
        post(bob, f"{S}/signup/delete", data={"id": sub_id.group(1)})

    # --- catalog
    r = post(root, "/admin/catalog/add", files={"apworld": ("bar.apworld", fixture("bar.apworld"))},
             data={"note": "vetted"})
    check("catalog add reads the manifest", "Added bar.apworld" in r.text and "Bar Finder" in r.text and "1.0.0" in r.text, r.text[-800:])
    sha = re.search(r'name="sha256" value="([0-9a-f]{64})"', r.text).group(1)
    page = get(bob, f"{S}/signup").text
    check("signup offers the catalog", f'value="{sha}"' in page and "Bar Finder 1.0.0 (bar.apworld)" in page)
    j = post(bob, f"{S}/signup/apworld", files={"catalog": (None, sha)}).json()
    check("catalog pick installs right away", j.get("ok") and not j.get("pending") and j.get("world") == "bar.apworld", j)
    adm = get(root, f"{S}/admin").text
    check("catalog install shows provenance", "from the catalog" in adm)
    r = post(bob, f"{S}/signup/check", files={"yaml": ("bar.yaml", fixture("bar.yaml"))}).json()
    check("catalog world checks", r.get("ok"), r)
    check("installed upload can be added to the catalog (button shown)", "add to catalog" in adm)
    post(root, f"{S}/admin/worlds/to-catalog", data={"name": "foo.apworld"})
    cat = get(root, "/admin/catalog").text
    check("season world added to the catalog", "foo.apworld" in cat and "Foo Finder" in cat)

    # a second season: an upload matching a catalog entry needs no approval
    post(root, "/admin/seasons", data=dict(name="Summer"))
    sid2 = max(int(x) for x in re.findall(r'/s/(\d+)/admin', get(root, "/admin/seasons").text))
    post(root, f"/s/{sid2}/admin/signups", data=dict(open="1"))
    j = post(alice, f"/s/{sid2}/signup/apworld", files={"apworld": ("foo.apworld", fixture("foo.apworld"))}).json()
    check("upload matching the catalog installs in manual mode", j.get("ok") and j.get("pending") is False, j)

    # --- auto
    post(root, "/admin/catalog/approval", data={"mode": "auto"})
    check("approval toggled to auto", 'value="auto" checked' in get(root, "/admin/catalog").text)
    j = post(alice, f"/s/{sid2}/signup/apworld", files={"apworld": ("baz.apworld", fixture("baz.apworld"))}).json()
    check("auto mode installs uploads", j.get("ok") and j.get("pending") is False, j)
    adm = get(root, f"/s/{sid2}/admin").text
    check("auto install provenance", "auto-approved" in adm)
    post(root, "/admin/catalog/approval", data={"mode": "manual"})

    # remove from catalog; installed copies stay
    post(root, "/admin/catalog/remove", data={"sha256": sha})
    check("removed from catalog", sha not in get(root, "/admin/catalog").text)
    check("season keeps its copy", "bar.apworld" in get(root, f"{S}/admin").text)

    # generate the first season now nothing is held
    post(root, f"{S}/admin/generate")
    done = wait_for(lambda: (t := get(root, f"{S}/admin").text) and ("Generated seed" in t or "Failed" in t) and t, 240, 3)
    m = re.search(r"(Generated seed[^<]*|Failed[^<]*)", done or "")
    check("generates once nothing is held", m and m.group(0).startswith("Generated"), m and m.group(0))
finally:
    srv.stop()

sys.exit(summary())
