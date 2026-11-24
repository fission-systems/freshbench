import math

from freshbench.bands import BAND_LABELS, band, normalized_ged


def test_band_edges():
    assert [band(n) for n in (0, 5, 6, 20, 21, 50, 51, 100, 101, 10_000)] == [
        "<6", "<6", "6-20", "6-20", "21-50", "21-50", "51-100", "51-100", ">100", ">100"]


def test_band_unknown():
    assert band(None) == "unknown"
    assert band(-1) == "unknown"


def test_labels_ordered():
    assert BAND_LABELS == ("<6", "6-20", "21-50", "51-100", ">100")


def test_normalized_ged():
    assert normalized_ged(0, 5, 6) == 0
    assert math.isclose(normalized_ged(11, 5, 6), 1.0)
    assert normalized_ged(None, 5, 6) is None
    assert normalized_ged(3, None, 6) is None
    assert normalized_ged(3, 0, 0) is None
