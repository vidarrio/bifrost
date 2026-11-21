"""The header's "home" pill comes from the instance settings (no built-in
operator link): absent by default, present on every page once set, labelled
HOME unless the sysadmin names it."""
import sys

from lib import Server, check, get, post, summary

srv = Server("homepill", port=18191, ap_ports="39169-39171")
srv.start()
try:
    root = srv.setup_sysadmin()
    page = get(root, "/admin/seasons").text
    check("no hard-coded home link in the page header", "vidarr" not in page and 'id="homeslot"' in page)
    check("no home link by default", get(root, "/api/me").json().get("home") == "")
    post(root, "/admin/settings", data=dict(domain="", home_url="https://home.example.org/", discords=""))
    check("/api/me carries the configured home link", get(root, "/api/me").json().get("home") == "https://home.example.org/")
    check("landing shows it too", 'href="https://home.example.org/"' in get(root, "/").text)
    check("…labelled HOME by default, with no arrow", '>HOME</a>' in get(root, "/").text and "↗" not in get(root, "/").text)
    check("/api/me carries the default label", get(root, "/api/me").json().get("home_label") == "HOME")
    post(root, "/admin/settings", data=dict(domain="", home_url="https://home.example.org/", home_label=" Our <site> ", discords=""))
    check("the label is its own setting", get(root, "/api/me").json().get("home_label") == "Our <site>")
    # Raw, not through get(), which decodes entities.
    check("…shown escaped on the landing page", 'href="https://home.example.org/">Our &lt;site&gt;</a>' in root.get(srv.base + "/").text)
    r = post(root, "/admin/settings", data=dict(domain="", home_url="https://home.example.org/", home_label="x" * 25, discords=""))
    check("a label over 24 characters is refused", "at most 24 characters" in r.text and get(root, "/api/me").json().get("home_label") == "Our <site>")
    r = get(srv.session(), "/api/me")
    check("anonymous header gets it as well", r.status_code == 401 and r.json().get("home") == "https://home.example.org/" and r.json().get("home_label") == "Our <site>", (r.status_code, r.text[:200]))
finally:
    srv.stop()

sys.exit(summary())
