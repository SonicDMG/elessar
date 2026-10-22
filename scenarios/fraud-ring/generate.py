#!/usr/bin/env python3
"""Generate the fraud-ring scenario's source files.

Deterministic (seeded), so the committed files are reproducible:

    python3 scenarios/fraud-ring/generate.py

The story: marketplace Lumen, 6-12 October 2026. Seller S-101 Brightline
Electronics runs a triangulation ring: about forty fresh buyer accounts on
three shared devices buy from Brightline (and its twin S-102, which shares
Brightline's payout account) with stolen cards and the WELCOME25 promo; the
real cardholders dispute the charges. Decoys: a risk-rules deploy on the 7th
spikes declines; cardholder tickets look like account takeover.

Dirt is planted on purpose and listed at the bottom of this file.
"""
import json
import os
import random
from datetime import datetime, timedelta, timezone

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "sources")
rng = random.Random(7)
TZ = timezone.utc
DAY0 = datetime(2026, 10, 6, tzinfo=TZ)


def at(day, hour, minute=0):
    return DAY0 + timedelta(days=day, hours=hour, minutes=minute)


def iso(t):
    return t.strftime("%Y-%m-%dT%H:%M:%SZ")


def us(t):
    return t.strftime("%m/%d/%Y %H:%M")


def dmy(t):
    return t.strftime("%d/%m/%Y")


def ms(t):
    return int(t.timestamp() * 1000)


def rand_time(day_lo, day_hi):
    secs = rng.randint(int(day_lo * 86400), int(day_hi * 86400))
    return DAY0 + timedelta(seconds=secs)


# ------------------------------------------------------------------ actors

FIRST = ["Ada", "Ben", "Chloe", "Dev", "Esme", "Farid", "Gwen", "Hiro", "Ines", "Jonas", "Kira", "Luis", "Maya", "Noor",
         "Otto", "Priya", "Quinn", "Rosa", "Sven", "Tara", "Umar", "Vera", "Wes", "Xia", "Yusuf", "Zoe"]
LAST = ["Alvarez", "Brooks", "Chen", "Dubois", "Evans", "Fischer", "Garcia", "Hassan", "Ito", "Jensen", "Kowalski",
        "Larsen", "Mendes", "Nakamura", "Okafor", "Petrov", "Quist", "Reyes", "Singh", "Tanaka", "Usman", "Varga",
        "Walsh", "Xu", "Yilmaz", "Zimmer"]
SELLER_WORDS = ["Nova", "Brightline", "Northwing", "Cobalt", "Harbor", "Juniper", "Kestrel", "Lumen", "Meridian",
                "Orchard", "Pioneer", "Quarry", "Redwood", "Summit", "Tidewater", "Umber", "Vantage", "Willow",
                "Yonder", "Zenith", "Amber", "Beacon", "Cinder", "Drift", "Ember", "Falcon", "Granite", "Hollow",
                "Ivory", "Jade"]
SELLER_KINDS = ["Goods", "Electronics Ltd", "Gadgets", "Supply Co", "Outfitters", "Trading", "Home", "Audio",
                "Parts", "Direct"]
SKUS = ["EARBUDS-X2", "SPEAKER-M", "CHARGER-65W", "DRONE-MINI", "WATCH-S3", "LAMP-ARC", "KETTLE-1.7", "BLENDER-PRO",
        "ROUTER-AX", "CAM-DASH", "KNIFE-SET", "YOGA-MAT", "BACKPACK-30L", "HEADSET-7", "CONSOLE-CTRL", "TABLET-10"]
COUNTRIES = ["US", "US", "US", "GB", "DE", "CA", "AU", "NL", "FR", "SG"]

sellers = []
for i in range(30):
    sid = f"S-{100 + i}"
    name = f"{SELLER_WORDS[i]} {SELLER_KINDS[i % len(SELLER_KINDS)]}"
    sellers.append({"id": sid, "name": name, "country": rng.choice(COUNTRIES),
                    "onboarded": at(-rng.randint(200, 1500), 9), "payout": f"PA-{rng.randrange(16**6):06x}",
                    "rating": round(rng.uniform(3.6, 4.9), 1), "status": "active"})
RING, TWIN, HONEST = sellers[1], sellers[2], sellers[0]
RING.update({"name": "Brightline Electronics Ltd", "country": "US", "onboarded": at(-41, 10)})
TWIN.update({"name": "Northwing Gadgets", "country": "US", "onboarded": at(-23, 15), "payout": RING["payout"]})
HONEST.update({"name": "Nova Goods", "country": "US", "rating": 4.8})
RING_NAME_VARIANTS = ["Brightline Electronics", "BRIGHTLINE ELEC.", "Brightline Electronics Ltd", "Brightline Elec"]

buyers = []
for i in range(230):
    f, l = rng.choice(FIRST), rng.choice(LAST)
    buyers.append({"id": f"B-{1000 + i}", "email": f"{f.lower()}.{l.lower()}{rng.randint(1, 99)}@example.com",
                   "age": rng.randint(30, 900), "device": f"fp-{rng.randrange(16**6):06x}",
                   "bin": rng.choice(["411111", "424242", "510510", "377711", "601100", "352800", "453201", "520082"])})
RING_DEVICES = ["fp-7c1e9a", "fp-21bb04", "fp-e0d5c7"]
RING_BINS = ["455612", "522094", "601199"]
ring_buyers = []
for i in range(40):
    f, l = rng.choice(FIRST), rng.choice(LAST)
    ring_buyers.append({"id": f"B-{2000 + i}", "email": f"{f.lower()}{l.lower()}{rng.randint(100, 999)}@mailbox.example",
                        "age": rng.randint(0, 2), "device": rng.choice(RING_DEVICES), "bin": rng.choice(RING_BINS)})
RING_BUYER = ring_buyers[17]
victims = []
for i in range(30):
    f, l = rng.choice(FIRST), rng.choice(LAST)
    victims.append({"email": f"{f.lower()}_{l.lower()}@example.net", "name": f"{f} {l}"})
VICTIM = victims[4]

# ------------------------------------------------------------------ orders

orders = []
oid = 50000


def new_order(t, buyer, seller, amount, promo, device=None):
    global oid
    oid += 1
    return {"id": f"O-{oid}", "t": t, "buyer": buyer, "seller": seller, "sku": rng.choice(SKUS), "amount": amount,
            "promo": promo, "device": device or buyer["device"], "ship": rng.choice(COUNTRIES)}


for _ in range(350):
    b = rng.choice(buyers)
    s = rng.choice(sellers)
    promo = "WELCOME25" if rng.random() < 0.07 else (rng.choice(["", "", "", "FALL10", "SHIP0"]))
    orders.append(new_order(rand_time(0, 6.9), b, s, round(rng.uniform(9, 320), 2), promo))
for _ in range(72):
    b = rng.choice(ring_buyers)
    s = RING if rng.random() < 0.8 else TWIN
    promo = "WELCOME25" if rng.random() < 0.85 else ""
    orders.append(new_order(rand_time(2.1, 4.9), b, s, round(rng.uniform(180, 420), 2), promo, b["device"]))
orders.sort(key=lambda o: o["t"])
ring_orders = [o for o in orders if o["seller"] in (RING, TWIN)]

# ------------------------------------------------------------------ writers

def write_orders():
    lines = ["order_id,placed_at,buyer_id,buyer_email,seller_id,seller_name,seller_country,sku,amount,currency,device_id,promo_code,ship_country,buyer_age_days"]
    rows = []
    for o in orders:
        t = us(o["t"]) if rng.random() < 0.05 else iso(o["t"])                 # dirt: 5% US-format timestamps
        buyer_id = "" if rng.random() < 0.04 else o["buyer"]["id"]            # dirt: missing buyer ids
        sname = rng.choice(RING_NAME_VARIANTS) if o["seller"] is RING else o["seller"]["name"]
        scountry = "SG" if o["seller"] is RING else o["seller"]["country"]    # dirt: ring seller claims SG here, US in registry
        currency = "$" if rng.random() < 0.1 else "USD"                       # dirt: mixed currency spelling
        amount = f"{o['amount']:.2f}"
        row = [o["id"], t, buyer_id, o["buyer"]["email"], o["seller"]["id"], sname, scountry, o["sku"], amount,
               currency, o["device"], o["promo"], o["ship"], str(o["buyer"]["age"])]
        rows.append(row)
    for i in rng.sample(range(len(rows)), 6):                                 # dirt: duplicate rows
        rows.append(list(rows[i]))
    def q(v):
        return f'"{v}"' if ("," in v or " " in v) else v
    for r in rows:
        lines.append(",".join(q(v) for v in r))
    last = lines[-1].split(",")[:5]                                           # dirt: truncated last row
    lines[-1] = ",".join(last)
    open(os.path.join(OUT, "orders.csv"), "w").write("\n".join(lines) + "\n")


def write_payments():
    lines = []
    txn = 90000
    recs = []
    for o in orders:
        if rng.random() < 0.12:
            continue                                                          # some orders have no payment record
        txn += 1
        t = o["t"] + timedelta(seconds=rng.randint(2, 40))
        v7 = t >= at(1, 9)
        ring = o["seller"] in (RING, TWIN)
        if ring:
            result = "approved" if rng.random() < 0.93 else "declined"
        else:
            result = "declined" if rng.random() < (0.12 if v7 else 0.04) else "approved"
        recs.append({"txn_id": f"T-{txn}", "order_id": o["id"],
                     "t": int(t.timestamp()) if rng.random() < 0.03 else ms(t),      # dirt: 3% seconds in an ms column
                     "card_bin": o["buyer"]["bin"], "last4": f"{rng.randint(0, 9999):04d}", "result": result,
                     "avs": ("N" if ring else rng.choice(["Y", "Y", "Y", "N"])) if rng.random() > 0.08 else "N/A",
                     "cvv_match": rng.choice([True, True, False]) if rng.random() > 0.05 else None,
                     "amount": o["amount"], "rules": "risk-rules-v7" if v7 else "risk-rules-v6"})
    for i in rng.sample(range(len(recs)), 8):                                 # dirt: duplicate lines
        recs.append(dict(recs[i]))
    for i, r in enumerate(recs):
        lines.append(json.dumps(r))
    lines.insert(len(lines) // 3, '{"txn_id": "T-9031X", "order_id": "O-50104", "t": 1791600000000, "card_bin": 411111, "result": approved}')  # dirt: malformed line
    open(os.path.join(OUT, "payments.jsonl"), "w").write("\n".join(lines) + "\n")


def write_devices():
    sessions = []
    sid = 7000
    seen = set()
    for o in orders:
        key = (o["buyer"]["id"], o["device"])
        if key in seen and rng.random() < 0.6:
            continue
        seen.add(key)
        sid += 1
        t = o["t"] - timedelta(minutes=rng.randint(1, 25)) + timedelta(hours=1)   # dirt: clock an hour ahead
        ring = o["buyer"] in ring_buyers
        sessions.append({"sessionId": f"sess-{sid}", "buyer": o["buyer"]["id"], "fingerprint": o["device"],
                         "ip": f"{rng.randint(11, 220)}.{rng.randint(0, 255)}.{rng.randint(0, 255)}.{rng.randint(1, 254)}",
                         "ua": rng.choice(["Chrome/130 Android", "Safari/18 iOS", "Chrome/130 Win", "Firefox/131 Mac"]),
                         "loginAt": iso(t), "geo": "ID" if ring else rng.choice(COUNTRIES)})
    ring_sessions = [x for x in sessions if x["geo"] == "ID"]
    other = [x for x in sessions if x["geo"] != "ID"]
    kept = ring_sessions + rng.sample(other, min(len(other), 150 - len(ring_sessions)))
    kept.sort(key=lambda x: x["loginAt"])
    doc = {"meta": {"collector": "fp-edge-01", "schemaVersion": "2.3", "window": {"from": iso(at(0, 0)), "to": iso(at(7, 0))},
                    "note": "loginAt is collector-local time"},
           "data": {"sessions": kept}}
    json.dump(doc, open(os.path.join(OUT, "sessions.json"), "w"), indent=1)


def write_tickets():
    tid = 3100
    items = []
    disputed = rng.sample(ring_orders, 34)
    for i, o in enumerate(disputed):
        tid += 1
        v = victims[i % len(victims)]
        opened = o["t"] + timedelta(hours=rng.randint(20, 90))
        text = rng.choice([
            "I did not make this purchase. I have never shopped with this seller.",
            "Unauthorized charge on my card for an order I never placed.",
            "This wasn't me. Please refund, my card was used without permission.",
            "I don't recognise this order at all. Not my account.",
        ])
        cat = "unauthorized" if rng.random() > 0.15 else "other"                 # dirt: some miscategorised
        items.append((tid, "dispute", cat, o, v["email"], opened, text, "P2"))
    for _ in range(30):
        tid += 1
        o = rng.choice(orders)
        b = o["buyer"]
        opened = o["t"] + timedelta(hours=rng.randint(24, 120))
        kind, cat, text = rng.choice([
            ("dispute", "shipping", "Order never arrived, tracking stuck for a week."),
            ("dispute", "quality", "Item not as described, the box was damaged."),
            ("question", "account", "How do I change my shipping address?"),
            ("complaint", "seller", "Seller stopped answering messages after payment."),
        ])
        items.append((tid, kind, cat, o, b["email"], opened, text, "P3"))
    for _ in range(14):
        tid += 1
        s = rng.choice(sellers)
        items.append((tid, "complaint", "payout", None, f"ops@{s['name'].split()[0].lower()}.example",
                      rand_time(0, 6.5), "Payout for last week is late, please advise.", "P3"))
    items.sort(key=lambda x: x[5])
    xml = ['<?xml version="1.0" encoding="UTF-8"?>', f'<helpdesk export="{iso(at(7, 8))}" tz="UTC">']
    xml.append(f'  <ticket id="CH-{tid + 1}" type="change" category="risk" priority="P2">\n'
               f'    <summary>Deploy risk-rules-v7 (velocity and AVS tightening)</summary>\n'
               f'    <ci type="model" name="Risk rules">risk-rules-v7</ci>\n'
               f'    <opened>{iso(at(1, 9))}</opened>\n    <status>completed</status>\n  </ticket>')
    for (t, kind, cat, o, email, opened, text, prio) in items:
        opened_xml = "" if rng.random() < 0.1 else f"\n    <opened>{iso(opened)}</opened>"   # dirt: missing opened
        order_xml = f'\n    <order id="{o["id"]}" seller="{o["seller"]["id"]}"/>' if o else ""
        xml.append(f'  <ticket id="TK-{t}" type="{kind}" category="{cat}" priority="{prio}">\n'
                   f'    <summary>{text}</summary>\n    <reporter email="{email}"/>{order_xml}{opened_xml}\n'
                   f'    <status>{rng.choice(["open", "open", "pending", "closed"])}</status>\n  </ticket>')
    xml.append("</helpdesk>")
    open(os.path.join(OUT, "tickets.xml"), "w").write("\n".join(xml) + "\n")


def write_chargebacks():
    rows = []
    cb = 400
    ring_cbs = rng.sample(ring_orders, 44)
    for o in ring_cbs:
        cb += 1
        filed = o["t"] + timedelta(days=rng.randint(2, 5))
        rows.append([f"CB-{cb}", o["id"], "10.4", dmy(filed), o["amount"], o["seller"]["id"]])
    for code, n in (("13.1", 9), ("13.3", 7)):
        for _ in range(n):
            cb += 1
            o = rng.choice([x for x in orders if x not in ring_orders])
            filed = o["t"] + timedelta(days=rng.randint(3, 6))
            rows.append([f"CB-{cb}", o["id"], code, dmy(filed), o["amount"], o["seller"]["id"]])
    for i in rng.sample(range(len(rows)), 4):
        rows[i][3] = "00/00/0000"                                               # dirt: placeholder dates
    rows[rng.randrange(len(rows))].pop()                                        # dirt: a short row
    rows.sort(key=lambda r: r[3])
    doc = {"source": "disputes-export", "schema": {"fields": ["cb_id", "order_id", "reason_code", "filed_at", "amount", "seller_id"]},
           "rows": rows}
    with open(os.path.join(OUT, "chargebacks.json"), "w") as f:
        f.write('{\n  "source": "disputes-export",\n  "schema": {"fields": ["cb_id", "order_id", "reason_code", "filed_at", "amount", "seller_id"]},\n  "rows": [\n')
        f.write(",\n".join("    " + json.dumps(r) for r in rows))
        f.write("\n  ]\n}\n")


def write_sellers():
    lines = ["SELLER_ID,LEGAL_NAME,COUNTRY,ONBOARDED,PAYOUT_ACCOUNT,RATING,STATUS"]
    for s in sellers:
        country = "-" if s is sellers[9] else s["country"]                      # dirt: placeholder
        rating = "unknown" if s is sellers[14] else str(s["rating"])            # dirt: placeholder
        lines.append(",".join([s["id"], f'"{s["name"]}"', country, dmy(s["onboarded"]), s["payout"], rating, s["status"]]))
    open(os.path.join(OUT, "sellers.csv"), "w").write("\n".join(lines) + "\n")


def write_promos():
    lines = ["# Promo catalog export · generated 2026-10-13T08:00:00Z", "# code=<id>; discount=<pct>; ..."]
    promos = [("WELCOME25", 25, "2026-10-06", "2026-10-20", 1, 5000),
              ("FALL10", 10, "2026-09-20", "2026-10-31", 3, 20000),
              ("SHIP0", 0, "2026-10-01", "2026-12-31", 10, 0),
              ("VIP15", 15, "2026-10-01", "2026-10-31", 2, 8000),
              ("AUDIO5", 5, "2026-10-10", "2026-10-17", 5, 1500),
              ("HOME20", 20, "2026-11-01", "2026-11-30", 1, 6000),
              ("REFER10", 10, "2026-01-01", "2026-12-31", 1, 50000),
              ("CLEAR30", 30, "2026-10-02", "2026-10-09", 1, 2000)]
    for code, pct, s, e, per, budget in promos:
        lines.append(f"code={code}; discount={pct}%; starts={s}; ends={e}; max_uses_per_account={per}; budget={budget}; new_accounts_only={'yes' if code in ('WELCOME25', 'REFER10') else 'no'}")
    lines.append("code=; discount=; starts=")                                   # dirt: empty trailing line
    open(os.path.join(OUT, "promos.txt"), "w").write("\n".join(lines) + "\n")


def write_partner():
    recs = []
    for b in RING_BINS:
        recs.append({"indicator": "bin", "value": b, "confidence": round(rng.uniform(0.72, 0.9), 2), "basis": "card-testing ring", "seen_at": iso(rand_time(2, 6))})
    for d in RING_DEVICES[:2]:
        recs.append({"indicator": "device", "value": d, "confidence": round(rng.uniform(0.8, 0.95), 2), "basis": "shared across marketplaces", "seen_at": iso(rand_time(3, 6))})
    recs.append({"indicator": "merchant", "value": "Brightline Electronics", "confidence": 0.71, "basis": "triangulation pattern", "country": "US", "seen_at": iso(at(5, 14))})
    recs.append({"indicator": "merchant", "value": "Nova Goods Co", "confidence": 0.55, "basis": "chargeback ratio", "country": "PH", "seen_at": iso(at(4, 11))})
    for b in rng.sample(ring_buyers, 8):
        recs.append({"indicator": "email", "value": b["email"], "confidence": round(rng.uniform(0.5, 0.8), 2), "basis": "disposable domain", "seen_at": iso(rand_time(2, 6))})
    for _ in range(10):
        recs.append({"indicator": rng.choice(["bin", "email", "device"]), "value": rng.choice([f"{rng.randint(400000, 599999)}", f"user{rng.randint(1, 999)}@throwaway.example", f"fp-{rng.randrange(16**6):06x}"]),
                     "confidence": round(rng.uniform(0.3, 0.7), 2), "basis": rng.choice(["velocity", "proxy", "reputation"]), "seen_at": iso(rand_time(0, 6))})
    for i in rng.sample(range(len(recs)), 3):
        recs[i].pop("seen_at")                                                  # dirt: undated indicators
    rng.shuffle(recs)
    open(os.path.join(OUT, "partner.jsonl"), "w").write("\n".join(json.dumps(r) for r in recs) + "\n")


if __name__ == "__main__":
    os.makedirs(OUT, exist_ok=True)
    write_orders(); write_payments(); write_devices(); write_tickets(); write_chargebacks(); write_sellers(); write_promos(); write_partner()
    print(f"ring seller {RING['id']} / twin {TWIN['id']} share payout {RING['payout']}; honest {HONEST['id']} {HONEST['name']}")
    print(f"ring buyer anchor {RING_BUYER['id']}; victim anchor {VICTIM['email']}; ring devices {RING_DEVICES}; ring bins {RING_BINS}")
    print(f"orders {len(orders)} (ring {len(ring_orders)})")

# Dirt planted:
#   orders.csv       5% US-format timestamps (undated), ~4% empty buyer ids, ring seller name
#                    variants, seller country SG here vs US in the registry, "$" vs "USD",
#                    6 duplicate rows, truncated last row
#   payments.jsonl   3% timestamps in seconds inside a milliseconds column (parse, but land in
#                    1970), one malformed JSON line, "N/A" and null placeholders, 8 duplicate lines,
#                    12% of orders have no payment record
#   sessions.json    collector clock an hour ahead
#   tickets.xml      10% missing <opened>, reporter as email not id, some disputes miscategorised
#   chargebacks.json 4 placeholder dates 00/00/0000, one short row
#   sellers.csv      "-" country and "unknown" rating; two sellers share a payout account
#   promos.txt       undated (document date in the header), one empty trailing record
#   partner.jsonl    3 indicators without seen_at; "Nova Goods Co" (PH) is not Nova Goods (US)
