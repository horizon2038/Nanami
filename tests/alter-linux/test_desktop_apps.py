"""Exercise the guest smoke test's real shell window selector with X11 output."""
from pathlib import Path
import subprocess
import tempfile
import unittest


SOURCE = Path(__file__).with_name("desktop-apps.sh").read_text()
SELECTOR = "find_viewable_window() {\n" + SOURCE.split(
    "find_viewable_window() {\n", 1)[1].split("\n}\n", 1)[0] + "\n}\n"


class WindowSelectionTests(unittest.TestCase):
    def select(self, tree, states, title="xfe|file explorer"):
        cases = []
        for window, state in states.items():
            self.assertRegex(window, r"^0x[0-9a-f]+$")
            if state is None:
                cases.append(f"{window}) return 1 ;;")
            else:
                self.assertIn(state, ("IsViewable", "IsUnMapped", "IsUnviewable"))
                cases.append(f"{window}) printf '  Map State: {state}\\n' ;;")
        mock = """
xwininfo() {
    printf '%s\n' "$2" >> "$probes"
    case "$2" in
""" + "\n".join(cases) + """
        *) return 1 ;;
    esac
}
tree=$1
title=$2
output=$3
probes=$4
find_viewable_window "$tree" "$title" "$output"
"""
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "tree").write_text(tree)
            result = subprocess.run(
                ["sh", "-c", SELECTOR + mock, "window-test", str(root / "tree"),
                 title, str(root / "output"), str(root / "probes")],
                capture_output=True, text=True, check=False)
            self.assertEqual(result.stderr, "")
            probes = (root / "probes").read_text().splitlines() if (root / "probes").exists() else []
            return result.returncode, probes

    def test_unnamed_helpers_before_real_title_do_not_match_wm_class(self):
        # Same IDs and ordering as the SMP4 timeout capture (57h1sn5s).
        tree = '''
     0x2005f1 (has no name): ("xfe" "Xfe")  52x23+0+0  +0+0
     0x2005f0 (has no name): ("xfe" "Xfe")  2x2+0+0  +0+0
        0x2003cd "Xfe (root) - /root": ("xfe" "Xfe")  740x460+3+21  +20+20
'''
        self.assertEqual(self.select(tree, {"0x2005f1": "IsUnMapped", "0x2003cd": "IsViewable"}),
                         (0, ["0x2003cd"]))

    def test_all_matching_titles_are_checked_after_unmapped_and_disappeared_windows(self):
        tree = '''
    0x10 "Xfe": ("xfe" "Xfe") 1x1+0+0
    0x11 "Xfe (root)": ("xfe" "Xfe") 1x1+0+0
    0x12 "File Explorer": ("xfe" "Xfe") 740x460+0+0
'''
        self.assertEqual(self.select(tree, {"0x10": "IsUnMapped", "0x11": None, "0x12": "IsViewable"}),
                         (0, ["0x10", "0x11", "0x12"]))

    def test_unmapped_or_unviewable_windows_never_pass(self):
        for state in ("IsUnMapped", "IsUnviewable"):
            with self.subTest(state=state):
                self.assertEqual(self.select('  0x10 "Xfe": ("xfe" "Xfe")\n', {"0x10": state}),
                                 (1, ["0x10"]))

    def test_unrelated_title_and_unnamed_class_are_rejected(self):
        for line in ('0x10 (has no name): ("xfe" "Xfe")',
                     '0x10 "Error loading icons": ("xfe" "Xfe")',
                     '0x10 "": ("xfe" "Xfe")'):
            with self.subTest(line=line):
                self.assertEqual(self.select(line + "\n", {"0x10": "IsViewable"}), (1, []))

    def test_every_application_title_matches_case_insensitively(self):
        for title, pattern in (("Xfe (root) - /root", "xfe|file explorer"),
                               ("logo.png", "logo.png"), ("mtPaint", "mtpaint"),
                               ("Galculator", "galculator"), ("FLIT", "flit")):
            with self.subTest(title=title):
                self.assertEqual(self.select(f'  0x10 "{title}": ("app" "App")\n',
                                             {"0x10": "IsViewable"}, pattern), (0, ["0x10"]))


if __name__ == "__main__":
    unittest.main()
