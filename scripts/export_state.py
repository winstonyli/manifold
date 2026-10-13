"""EXPORTSTATE card helpers shared by the probe scripts (chrome-shots.sh,
json-level-probe.py).

An EXPORTSTATE `cards[]` entry names a card by its type (plan 2b-6, Ruling 9):
  type       the card's MIME type (the TYPE_* constants in store/model.rs)
  loaded     true only for a tab with a running engine
  json_card  true for a JSON level's card, which is no stored node
  unresponsive  true for a live tab whose engine stopped reading its pipe (wave 3b)
The old `kind` enum names (Tab, Unloaded, Note, ...) are gone; this module is
the one place that spells the mapping, so a script says `card_is(c, TAB, True)`
for a live tab and `card_is(c, TAB, False)` for a sleeping one.

Inline `python -P -c` snippets import it with PYTHONPATH set to scripts/ (the
bash scripts export that); json-level-probe.py adds its own directory.
"""

TAB = "application/vnd.manifold.tab"
GROUP = "application/vnd.manifold.group"
NOTE = "text/markdown"
IMAGE = "image/png"
PDF = "application/pdf"
JSON = "application/json"
DOWNLOAD = "application/octet-stream"


def card_is(card, ty, loaded=None):
    """Whether `card` is a stored node of type `ty`; `loaded` True or False also
    requires that engine state (a live tab, or a sleeping/waking/failed one).
    A JSON level's card is never a stored node."""
    if card["json_card"] or card["type"] != ty:
        return False
    return loaded is None or card["loaded"] == loaded


def label(card):
    """One token naming the card, for a probe's expected-vs-got line: its type,
    `;loaded` added for a live tab, `json-card` for a JSON level's card."""
    if card["json_card"]:
        return "json-card"
    return card["type"] + (";loaded" if card["loaded"] else "")
