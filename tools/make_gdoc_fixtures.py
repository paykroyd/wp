#!/usr/bin/env python3
"""Generate Google Docs API fixtures (`documents.get` responses) for
crates/wp-gdoc/tests. Indexes are UTF-16 offsets, computed here so the
fixtures stay exact.

    python3 tools/make_gdoc_fixtures.py crates/wp-gdoc/tests/fixtures
"""
import json
import sys

out_dir = sys.argv[1] if len(sys.argv) > 1 else "crates/wp-gdoc/tests/fixtures"


def u16(s):
    return len(s.encode("utf-16-le")) // 2


def pt(n):
    return {"magnitude": n, "unit": "PT"}


class Seg:
    """A body or footnote: structural elements with running indexes."""

    def __init__(self, start=0):
        self.i = start
        self.content = []

    def section_break(self):
        el = {"startIndex": self.i, "endIndex": self.i + 1, "sectionBreak": {"sectionStyle": {"columnSeparatorStyle": "NONE", "contentDirection": "LEFT_TO_RIGHT", "sectionType": "CONTINUOUS"}}}
        self.content.append(el)
        self.i += 1

    def para(self, runs, style=None, bullet=None):
        """runs: list of (text, textStyle) or dicts for non-text elements.
        The paragraph's newline is appended to the last text run."""
        start = self.i
        elements = []
        items = list(runs)
        if items and isinstance(items[-1], tuple):
            t, ts = items[-1]
            items[-1] = (t + "\n", ts)
        else:
            items.append(("\n", {}))
        for it in items:
            if isinstance(it, tuple):
                t, ts = it
                run = {"content": t, "textStyle": {k: v for k, v in ts.items() if not k.startswith("suggested")}}
                run.update({k: v for k, v in ts.items() if k.startswith("suggested")})
                el = {"startIndex": self.i, "endIndex": self.i + u16(t), "textRun": run}
                self.i += u16(t)
            else:
                el = dict(it)
                n = el.pop("_len", 1)
                el = {"startIndex": self.i, "endIndex": self.i + n, **el}
                self.i += n
            elements.append(el)
        ps = {"namedStyleType": "NORMAL_TEXT", "direction": "LEFT_TO_RIGHT"}
        if style:
            ps.update(style)
        p = {"elements": elements, "paragraphStyle": ps}
        if bullet:
            p["bullet"] = bullet
        self.content.append({"startIndex": start, "endIndex": self.i, "paragraph": p})
        return start

    def table(self, rows, widths):
        """rows: list of lists of cell paragraph text."""
        start = self.i
        self.i += 1  # table start
        trs = []
        for r in rows:
            rstart = self.i
            self.i += 1  # row start
            tcs = []
            for text in r:
                cstart = self.i
                self.i += 1  # cell start
                cell = Seg(self.i)
                cell.para([(text, {})])
                self.i = cell.i
                tcs.append({"startIndex": cstart, "endIndex": self.i, "content": cell.content, "tableCellStyle": {"rowSpan": 1, "columnSpan": 1, "backgroundColor": {}, "paddingLeft": pt(5), "paddingRight": pt(5), "paddingTop": pt(5), "paddingBottom": pt(5), "contentAlignment": "TOP"}})
            trs.append({"startIndex": rstart, "endIndex": self.i, "tableCells": tcs, "tableRowStyle": {"minRowHeight": pt(0)}})
        self.content.append({"startIndex": start, "endIndex": self.i, "table": {"rows": len(rows), "columns": len(rows[0]), "tableRows": trs, "tableStyle": {"tableColumnProperties": [{"widthType": "FIXED_WIDTH", "width": pt(w)} for w in widths]}}})
        return start


BOLD = {"bold": True}
LINK = {"underline": True, "foregroundColor": {"color": {"rgbColor": {"red": 0.06666667, "green": 0.33333334, "blue": 0.8}}}, "link": {"url": "https://example.com/report"}}
NAMED_STYLES = {
    "styles": [
        {"namedStyleType": "NORMAL_TEXT", "textStyle": {"bold": False, "italic": False, "underline": False, "strikethrough": False, "smallCaps": False, "backgroundColor": {}, "foregroundColor": {"color": {"rgbColor": {}}}, "fontSize": pt(11), "weightedFontFamily": {"fontFamily": "Arial", "weight": 400}, "baselineOffset": "NONE"}, "paragraphStyle": {"namedStyleType": "NORMAL_TEXT", "alignment": "START", "lineSpacing": 115, "direction": "LEFT_TO_RIGHT", "spacingMode": "COLLAPSE_LISTS", "spaceAbove": pt(0), "spaceBelow": pt(0), "avoidWidowAndOrphan": True}},
        {"namedStyleType": "HEADING_1", "textStyle": {"fontSize": pt(20)}, "paragraphStyle": {"namedStyleType": "NORMAL_TEXT", "direction": "LEFT_TO_RIGHT", "spaceAbove": pt(20), "spaceBelow": pt(6), "keepLinesTogether": True, "keepWithNext": True}},
        {"namedStyleType": "HEADING_2", "textStyle": {"fontSize": pt(16)}, "paragraphStyle": {"namedStyleType": "NORMAL_TEXT", "direction": "LEFT_TO_RIGHT", "spaceAbove": pt(18), "spaceBelow": pt(6), "keepLinesTogether": True, "keepWithNext": True}},
        {"namedStyleType": "TITLE", "textStyle": {"fontSize": pt(26)}, "paragraphStyle": {"namedStyleType": "NORMAL_TEXT", "direction": "LEFT_TO_RIGHT", "spaceAbove": pt(0), "spaceBelow": pt(3), "keepLinesTogether": True, "keepWithNext": True}},
    ]
}
DOC_STYLE = {"background": {"color": {}}, "pageNumberStart": 1, "marginTop": pt(72), "marginBottom": pt(72), "marginRight": pt(72), "marginLeft": pt(72), "pageSize": {"height": pt(792), "width": pt(612)}, "marginHeader": pt(36), "marginFooter": pt(36), "useCustomHeaderFooterMargins": True}


def report(tabs=False):
    body = Seg()
    body.section_break()
    body.para([("Quarterly Report", {})], style={"namedStyleType": "HEADING_1", "headingId": "h.abc123"})
    body.para([("The ", {}), ("results", BOLD), (" were ", {}), ("published", LINK), (".", {}), {"footnoteReference": {"footnoteId": "kix.fn1", "footnoteNumber": "1", "textStyle": {"baselineOffset": "SUPERSCRIPT"}}}])
    bullet = lambda: {"listId": "kix.list1", "textStyle": {"underline": False}}
    body.para([("Revenue up", {})], style={"indentFirstLine": pt(18), "indentStart": pt(36)}, bullet=bullet())
    body.para([("Costs down", {})], style={"indentFirstLine": pt(18), "indentStart": pt(36)}, bullet=bullet())
    # A tab and a soft line break (Docs stores it as U+000B).
    body.para([("Name:\tValueSecond line", {})], style={"alignment": "CENTER", "tabStops": [{"offset": pt(144), "alignment": "START"}]})
    body.table([["A1", "B1"], ["A2", "B2"]], [234, 234])
    body.para([("Figure: ", {}), {"inlineObjectElement": {"inlineObjectId": "kix.img1", "textStyle": {}}}])
    body.para([{"pageBreak": {"textStyle": {}}}])
    body.para([("Café — naïve \U0001F600 end.", {"italic": True, "fontSize": pt(14), "foregroundColor": {"color": {"rgbColor": {"red": 1}}}, "backgroundColor": {"color": {"rgbColor": {"red": 1, "green": 1}}}})], style={"spaceAbove": pt(12), "lineSpacing": 150})
    fn = Seg(0)
    fn.para([("Source: ", {}), ("annual filing", {"italic": True}), (".", {})])
    tab = {
        "body": {"content": body.content},
        "footnotes": {"kix.fn1": {"footnoteId": "kix.fn1", "content": fn.content}},
        "lists": {"kix.list1": {"listProperties": {"nestingLevels": [{"bulletAlignment": "START", "glyphSymbol": "●", "glyphFormat": "%0", "indentFirstLine": pt(18), "indentStart": pt(36), "textStyle": {"underline": False}, "startNumber": 1}] + [{"bulletAlignment": "START", "glyphSymbol": "○", "glyphFormat": "%1", "indentFirstLine": pt(54), "indentStart": pt(72), "startNumber": 1} for _ in range(8)]}}},
        "namedStyles": NAMED_STYLES,
        "documentStyle": DOC_STYLE,
        "inlineObjects": {"kix.img1": {"objectId": "kix.img1", "inlineObjectProperties": {"embeddedObject": {"imageProperties": {"contentUri": "https://lh7.example/img", "cropProperties": {}}, "size": {"height": pt(120), "width": pt(200)}, "marginTop": pt(9), "marginBottom": pt(9), "marginRight": pt(9), "marginLeft": pt(9)}}}},
        "suggestionsViewMode": "SUGGESTIONS_INLINE",
    }
    doc = {"documentId": "1AbCdEfGhIjKlMnOpQrStUvWxYz", "title": "Quarterly Report", "revisionId": "ALBJ4LtRev1"}
    if tabs:
        doc["tabs"] = [{"tabProperties": {"tabId": "t.0", "title": "Tab 1", "index": 0}, "documentTab": tab}]
    else:
        doc.update(tab)
    return doc


def numbered():
    body = Seg()
    body.section_break()
    body.para([("Steps", {})], style={"namedStyleType": "HEADING_2"})
    num = lambda lvl: {"listId": "kix.num", "nestingLevel": lvl} if lvl else {"listId": "kix.num"}
    body.para([("First", {})], style={"indentFirstLine": pt(18), "indentStart": pt(36)}, bullet=num(0))
    body.para([("Nested", {})], style={"indentFirstLine": pt(54), "indentStart": pt(72)}, bullet=num(1))
    body.para([("Second", {})], style={"indentFirstLine": pt(18), "indentStart": pt(36)}, bullet=num(0))
    body.para([("Plain ", {}), ("inserted", {"suggestedInsertionIds": ["suggest.1"]}), (" text", {})])
    body.para([("Keep ", {}), ("gone", {"suggestedDeletionIds": ["suggest.2"]}), (" this", {})])
    body.para([("Done", {})])
    return {
        "documentId": "1Numbered",
        "title": "Steps",
        "revisionId": "ALBJ4LtRev9",
        "body": {"content": body.content},
        "lists": {"kix.num": {"listProperties": {"nestingLevels": [{"bulletAlignment": "START", "glyphType": "DECIMAL", "glyphFormat": "%0.", "indentFirstLine": pt(18), "indentStart": pt(36), "startNumber": 1}, {"bulletAlignment": "START", "glyphType": "ALPHA", "glyphFormat": "%1.", "indentFirstLine": pt(54), "indentStart": pt(72), "startNumber": 1}]}}},
        "namedStyles": NAMED_STYLES,
        "documentStyle": DOC_STYLE,
    }


for name, doc in [("report.json", report()), ("report-tabs.json", report(tabs=True)), ("numbered.json", numbered())]:
    with open(f"{out_dir}/{name}", "w") as f:
        json.dump(doc, f, indent=1, ensure_ascii=False)
        f.write("\n")
    print(name)
