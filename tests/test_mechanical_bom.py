"""The BOM's views on an assembly whose answers are known by hand.

root
  SubA x2 (no bodies: a phantom)          each holds Plate x2 (made) and Kit x1 (purchased, not exploded)
  2020 x 300 x2, 2020 x 500 x1            extrusions for the cut list
  BHCS M5x8 x4                            fasteners
"""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("bom_views", Path(__file__).parents[1] / "addin/AdomBridge/commands/mechanical/bom_views.py")
v = importlib.util.module_from_spec(spec)
spec.loader.exec_module(v)


def meta(name, **extra):
    return {"name": name, "partNumber": extra.pop("partNumber", ""), "description": "", **extra}


def inst(path, parent, cid, m, bodies=1, children=0, purchased=False):
    return {"path": path, "parentPath": parent, "componentId": cid, "metadata": m, "bodyCount": bodies,
            "childCount": children, "asPurchased": purchased}


SUB, PLATE, KIT = meta("SubA", partNumber="2026-10-28-13-58-44-431"), meta("Plate", material={"name": "Aluminum 6061"}), meta("Kit", asPurchased=True, supplierPartNumber="K-1", unitCost=12.5)
E300, E500 = meta("2020 x 300", stock="2020"), meta("2020 x 500")
SCREW = meta("BHCS M5x8", partNumber="91239A120", supplierPartNumber="91239A120", unitCost=0.2)
INSTANCES = [
    inst("SubA:1", "<root>", "sub", SUB, bodies=0, children=3),
    inst("SubA:1+Plate:1", "SubA:1", "plate", PLATE), inst("SubA:1+Plate:2", "SubA:1", "plate", PLATE),
    inst("SubA:1+Kit:1", "SubA:1", "kit", KIT, children=2, purchased=True),
    inst("SubA:2", "<root>", "sub", SUB, bodies=0, children=3),
    inst("SubA:2+Plate:1", "SubA:2", "plate", PLATE), inst("SubA:2+Plate:2", "SubA:2", "plate", PLATE),
    inst("SubA:2+Kit:1", "SubA:2", "kit", KIT, children=2, purchased=True),
    inst("E300:1", "<root>", "e300", E300), inst("E300:2", "<root>", "e300", E300),
    inst("E500:1", "<root>", "e500", E500),
] + [inst("BHCS:%d" % i, "<root>", "screw", SCREW) for i in range(1, 5)]
PARTS = [
    {"componentId": "plate", **PLATE, "quantity": 4, "scope": "direct_component_bodies", "unitMass": 0.1},
    {"componentId": "kit", **KIT, "quantity": 2, "scope": "component_subtree", "unitMass": 0.05},
    {"componentId": "e300", **E300, "quantity": 2, "scope": "direct_component_bodies", "unitMass": 0.15},
    {"componentId": "e500", **E500, "quantity": 1, "scope": "direct_component_bodies", "unitMass": 0.25},
    {"componentId": "screw", **SCREW, "quantity": 4, "scope": "direct_component_bodies", "unitMass": 0.002},
]
LENGTHS = {"e300": 300.04, "e500": 499.98}


class BomViews(unittest.TestCase):
    def setUp(self):
        self.flat = v.flat(PARTS, LENGTHS)
        self.by = {l["componentId"]: l for l in self.flat}

    def test_indented_numbers_and_rolled_up_quantities(self):
        lines = {l["itemNumber"]: l for l in v.indented(INSTANCES, PARTS)}
        self.assertEqual(sorted(lines), ["1", "1.1", "1.2", "2", "3", "4"])
        self.assertEqual((lines["1"]["name"], lines["1"]["kind"], lines["1"]["qtyPerParent"], lines["1"]["totalQty"]), ("SubA", "phantom", 2, 2))
        self.assertEqual((lines["1.1"]["name"], lines["1.1"]["qtyPerParent"], lines["1.1"]["totalQty"], lines["1.1"]["level"]), ("Plate", 2, 4, 2))
        self.assertEqual((lines["1.2"]["kind"], lines["1.2"]["totalQty"]), ("kit", 2))
        self.assertEqual((lines["4"]["name"], lines["4"]["totalQty"], lines["4"]["makeBuy"], lines["4"]["category"]), ("BHCS M5x8", 4, "buy", "fastener"))

    def test_flat_make_buy_category_and_cost(self):
        self.assertEqual(self.by["plate"]["makeBuy"], "make")
        self.assertEqual(self.by["kit"]["makeBuy"], "buy")
        self.assertEqual((self.by["screw"]["category"], self.by["screw"]["makeBuy"], self.by["screw"]["extendedCost"]), ("fastener", "buy", 0.8))
        self.assertEqual((self.by["e300"]["category"], self.by["e300"]["stock"], self.by["e500"]["stock"]), ("extrusion", "2020", "2020"))
        self.assertAlmostEqual(self.by["plate"]["totalMassKg"], 0.4)

    def test_cut_list_groups_profile_and_length(self):
        cuts = v.cut_list(self.flat)
        self.assertEqual([(c["profile"], c["lengthMm"], c["quantity"]) for c in cuts], [("2020", 500.0, 1), ("2020", 300.0, 2)])

    def test_bar_plan_first_fit_decreasing_with_kerf(self):
        cuts = [{"profile": "2020", "lengthMm": 300.0, "quantity": 4, "components": []},
                {"profile": "2020", "lengthMm": 500.0, "quantity": 1, "components": []},
                {"profile": "2020", "lengthMm": 1200.0, "quantity": 1, "components": []}]
        plan = v.stock_plan(cuts, 1000, kerf=3)["2020"]
        self.assertEqual(plan["bars"], 2)
        self.assertEqual(plan["cutsPerBar"], [[500.0, 300.0], [300.0, 300.0, 300.0]])
        self.assertEqual(plan["offcutsMm"], [197.0, 94.0])
        self.assertEqual(plan["tooLong"], [1200.0])

    def test_fasteners_roll_up_by_part_number(self):
        self.assertEqual(v.fastener_rollup(self.flat), [{"spec": "91239A120", "quantity": 4, "names": ["BHCS M5x8"], "supplierPartNumber": "91239A120"}])

    def test_warnings_say_what_blocks_ordering(self):
        codes = {(w["code"], w["component"]) for w in v.warnings(self.flat)}
        self.assertIn(("make_without_material", "2020 x 300"), codes)
        self.assertNotIn(("buy_without_cost", "Kit"), codes)
        bad = v.flat([{"componentId": "c", "name": "Component7", "quantity": 1, "scope": "component_subtree", "asPurchased": True, "unitMass": 0}])
        self.assertEqual({w["code"] for w in v.warnings(bad)}, {"default_name", "no_part_number", "buy_without_supplier_part_number", "buy_without_cost"})

    def test_fusions_automatic_part_number_is_not_a_part_number(self):
        # Fusion fills new components with a timestamp part number; a grouping with one is still a phantom
        self.assertEqual(v.part_number({"name": "SubA", "partNumber": "2026-10-28-13-58-44-431"}), "")
        self.assertEqual(v.part_number({"name": "Bracket", "partNumber": "Bracket"}), "")
        self.assertEqual(v.part_number({"name": "Bracket", "partNumber": "ADM-0042"}), "ADM-0042")
        bought = v.flat([{"componentId": "c", "name": "Clamp", "partNumber": "2026-10-28-13-58-44-431", "quantity": 1,
                          "scope": "component_subtree", "asPurchased": True, "unitMass": 0}])
        self.assertIn("no_part_number", {w["code"] for w in v.warnings(bought)})

    def test_totals_cover_cost_and_mass(self):
        t = v.totals(self.flat)
        self.assertEqual((t["lines"], t["buyLines"], t["makeLines"], t["totalQuantity"]), (5, 2, 3, 13))
        self.assertEqual(t["costCoverage"], 1.0)
        self.assertEqual(t["extendedCost"], {"USD": 25.8})



def part(name, bodies=1, **extra):
    return {"componentId": extra.pop("componentId", name), "name": name, "partNumber": "", "quantity": extra.pop("quantity", 1),
            "scope": "direct_component_bodies" if bodies else "component_subtree", "bodyCount": bodies, "unitMass": 0, **extra}


class NameRules(unittest.TestCase):
    """What the BOM reads from names when nobody set metadata, each rule from a line a 3,000-part cell
    got wrong: every rule says why on the line, and an explicit field always wins."""

    def line(self, name, **extra):
        return v.flat([part(name, **extra)])[0]

    def test_hardware_wins_over_a_profile_size_in_the_name(self):
        for name, expected in {"T Nut Drop In 2020 M5": "fastener", "2020 Right Angle External Bkt": "hardware",
                               "TR_FASTENINGS_LTD-M2_5X8MM_X__1": "fastener", "BHCS_M5x8": "fastener", "L_Bracket": "hardware",
                               "40-4040 x 80 arm adapter (80/20)": "extrusion", "Peanut tray": None, "LM2596 module": None}.items():
            self.assertEqual(v.category({"name": name}), expected, name)
        # a piece of extrusion cut to length is made here, even with the maker in its name
        self.assertEqual(self.line("40-4040 x 80 arm adapter (80/20)")["makeBuy"], "make")

    def test_footprints_and_boards_are_electronics(self):
        for name in ("R_0603_1608METRIC-955", "C_0402_1005METRIC-68721", "SOT_23_5", "D_SOD_123", "HL2313_KICAD_PCB"):
            self.assertEqual(v.category({"name": name}), "electronics", name)
        self.assertIsNone(v.category({"name": "U channel"}))

    def test_mcmaster_numbers_become_supplier_part_numbers(self):
        line = self.line("Terminal block 7641K921 5mm gray")
        self.assertEqual((line["makeBuy"], line["supplier"], line["supplierPartNumber"], line["supplierInferred"]),
                         ("buy", "McMaster-Carr", "7641K921", True))
        self.assertIsNone(v.inferred_supplier({"name": "14INCH-2160X1440-TP"})[1])
        # cut from a catalog sheet: made here, and the catalog number is its stock
        made = self.line("Bench top HDPE 5/8 in (cut from 8619K87)")
        self.assertEqual((made["makeBuy"], made["stock"], made["supplierPartNumber"]), ("make", "8619K87", None))
        # another supplier's own number is never read as McMaster's
        self.assertEqual(v.inferred_supplier({"name": "Rail 1234K56", "supplier": "Misumi"}), ("Misumi", None))

    def test_make_or_buy_says_why(self):
        for name, expected in {"Breaker Eaton FAZ-C20-2-NA-L": ("buy", "maker named: Eaton"),
                               "Orange Pi Mount": ("make", "modeled bodies"),
                               "Base plate 200x200x6 (fabricated)": ("make", "named as made here"),
                               "Scale V1.1 (vendor STL)": ("buy", "vendor note (vendor STL)"),
                               "Standoff M5 110mm 3dp": ("make", "named as made here"),
                               "Water bottle 4280T23": ("buy", "McMaster-Carr number in the name")}.items():
            line = self.line(name)
            self.assertEqual((line["makeBuy"], line["makeBuyReason"]), expected, name)
        self.assertEqual(self.line("Breaker Eaton FAZ-C20", makeBuy="make")["makeBuyReason"], "metadata")

    def test_assemblies_that_are_bought_products_count_once(self):
        self.assertEqual(v.inferred_kit("Pump unit (vendor STEP V1.4)", ["Chassis", "Group head"]), "vendor note (vendor STEP V1.4)")
        self.assertEqual(v.inferred_kit("ZB5AW363.stp", ["a", "b"]), "imported model")
        self.assertEqual(v.inferred_kit("Open CASCADE STEP translator 7.9 1", ["a"]), "imported model")
        self.assertEqual(v.inferred_kit("Controller", ["PCB", "Board", "R16", "C5", "U8"]), "circuit board")
        self.assertEqual(v.inferred_kit("Swing arm 4259N11 (posable)", ["base", "link", "end"]), "McMaster-Carr 4259N11 in the name")
        self.assertIsNone(v.inferred_kit("Standoff M5 110mm 3dp", ["Standoff body", "BHCS M5x8"]))
        self.assertIsNone(v.inferred_kit("Frame", ["2020 x 300", "2020 x 500", "Corner bracket"]))
        kit = v.flat([part("Pump unit (vendor STEP V1.4)", bodies=0, kitReason="vendor note (vendor STEP V1.4)")])[0]
        self.assertEqual((kit["makeBuy"], kit["asPurchased"]), ("buy", True))

    def test_a_library_group_is_not_part_of_the_build(self):
        self.assertTrue(v.is_library("Vendor library", 0))
        self.assertFalse(v.is_library("Vendor library", 3))
        self.assertFalse(v.is_library("User_Library-SOT-23-5", 0))

    def test_wires_group_by_gauge_and_color(self):
        names = ["W001 1L1 12AWG black XD1:L-Q1:1", "W004 1L2 12AWG black Q1:2-K1:1", "W095 SL-R 18AWG black X4:14-H1:red lead",
                 "Ferrule 12 AWG 7950K31"]
        lines = v.flat([part(n) for n in names], {names[0]: 1116.8, names[1]: 591.7, names[2]: 400.0})
        self.assertEqual([(l["category"], l["makeBuy"]) for l in lines], [("wire", "make")] * 3 + [("cable", "buy")])
        groups = v.wire_list(lines)
        self.assertEqual([(g["gauge"], g["color"], g["count"], g["totalLengthMm"]) for g in groups], [("12", "black", 2, 1708.5), ("18", "black", 1, 400.0)])
        self.assertEqual(groups[0]["wires"][0]["wireId"], "W001")

    def test_the_order_list_merges_one_catalog_number(self):
        lines = v.flat([part("Cable tie 7130K59 loop %d" % i, unitCost=0.1) for i in range(3)] + [part("Breaker Eaton FAZ-C20", quantity=2)])
        order = v.order_list(lines)
        self.assertEqual([(o["supplier"], o["orderNumber"], o["quantity"], o["lines"], o["extendedCost"], o["inferred"]) for o in order],
                         [("McMaster-Carr", "7130K59", 3, 3, 0.3, True), (None, "Breaker Eaton FAZ-C20", 2, 1, None, True)])
        self.assertEqual(v.totals(lines)["inferredLines"], 4)

    def test_short_extrusions_stay_off_the_cut_list(self):
        lines = v.flat([part("TS-1010 slot cover", quantity=4)], {"TS-1010 slot cover": 4.5})
        self.assertEqual(v.cut_list(lines), [])
        self.assertIn("short_cut", {w["code"] for w in v.warnings(lines)})

    def test_rail_and_duct_are_cut_from_stock_like_extrusion(self):
        names = ["DIN-rail 35x7.5x715", "PVC-DUCT-25X40-L-1000", "PVC-DUCT-25X40-L-1000-COVER"]
        lines = v.flat([part(n, quantity=q) for n, q in zip(names, (3, 2, 2))], {names[0]: 715.0, names[1]: 1000.0, names[2]: 1000.0})
        self.assertEqual([(l["category"], l["makeBuy"], l["stock"]) for l in lines],
                         [("din rail", "make", "DIN rail 35x7.5"), ("duct", "make", "duct 25x40"), ("duct", "make", "duct 25x40 cover")])
        cuts = v.cut_list(lines)
        self.assertEqual([(c["profile"], c["lengthMm"], c["quantity"]) for c in cuts],
                         [("DIN rail 35x7.5", 715.0, 3), ("duct 25x40", 1000.0, 2), ("duct 25x40 cover", 1000.0, 2)])
        # the bar plan is for extrusion sticks; rail and duct come in other lengths
        self.assertEqual(v.stock_plan(cuts, 3683), {})
        self.assertNotIn("make_without_material", {w["code"] for w in v.warnings(v.flat([part("W001 18AWG red")]))})

    def test_cords_are_bought_and_numbered_wires_are_cut(self):
        for name, expected in {"W1 Cord NEMA 5-20P to IEC C19, 8 ft, 12/3 SJT": ("cable", "buy"),
                               "W-ETH-UP Cat6 patch cord 4.5 m": ("cable", "buy"),
                               "W106 12V1 16AWG black X5:1-E2:DC plug + (Adafruit 369)": ("wire", "make"),
                               "W104 5V2 16AWG black X3:2-E1:USB-C pigtail VBUS (splice)": ("wire", "make"),
                               "Anchor fastener 40mm": ("fastener", "buy"), "Diagonal brace 40mm 305": ("hardware", "buy")}.items():
            line = self.line(name)
            self.assertEqual((line["category"], line["makeBuy"]), expected, name)

    def test_borrowed_and_supplied_items_are_ordered_by_nobody(self):
        for name, reason in {"Machine cord end: IEC C13 connector (stand-in from 71535K205)": "stand-in geometry",
                             "W1 end: NEMA 5-20P plug (from 5512N11)": "stand-in geometry",
                             "Machine power cord, supplied with the machine": "supplied with another item",
                             "Kit outlet tube, 5 mm ID silicone (supplie": "supplied with another item"}.items():
            line = self.line(name)
            self.assertEqual((line["makeBuy"], line["makeBuyReason"], line["supplierPartNumber"]), (None, reason, None), name)
        self.assertEqual(v.inferred_kit("W2 end: IEC C13 connector (from 71535K205)", ["pin", "housing"]), "stand-in geometry")
        kit = v.flat([part("W2 end: IEC C13 connector (from 71535K205)", bodies=0, kitReason="stand-in geometry")])
        self.assertEqual((kit[0]["makeBuy"], kit[0]["asPurchased"], v.order_list(kit)), (None, False, []))
        self.assertIn("stand_in", {w["code"] for w in v.warnings(kit)})
        # cut from a catalog bar: made here, the bar is the stock
        jumper = self.line("Bar jumper X1:1-2 (2 poles cut from 7641K925, modelled)")
        self.assertEqual((jumper["makeBuy"], jumper["stock"]), ("make", "7641K925"))
        self.assertEqual(self.line("Supplies tote 41195T131")["makeBuy"], "buy")

    def test_default_names_and_body_heavy_parts_are_flagged(self):
        codes = {(w["code"], w["component"]) for w in v.warnings(v.flat([part("Extruded_3"), part("Mannequin", bodies=597)]))}
        self.assertIn(("default_name", "Extruded_3"), codes)
        self.assertIn(("make_with_many_bodies", "Mannequin"), codes)


if __name__ == "__main__":
    unittest.main()
