#!/usr/bin/env python3
"""Generate the sample lake that ships in `data/`.

The data is deterministic (fixed seed) and deliberately dirty: the checks in
`rules/quality.json` are written against the specific defects planted here, so a
failing rule is a feature of the demo rather than an accident.

    python3 scripts/gen_sample_data.py
"""

from __future__ import annotations

import csv
import random
from datetime import date, timedelta
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DATA = ROOT / "data"

CHANNELS = ["web", "ios", "android", "partner_api"]
STATUSES = ["paid", "pending", "refunded", "cancelled"]
COUNTRIES = ["US", "DE", "BR", "JP", "FR", "CA", "AU", "SG", "GB", "MX"]
CATEGORIES = ["hardware", "software", "subscription", "services", "training"]
FIRST = [
    "Ana", "Bruno", "Chen", "Dana", "Elias", "Fatima", "Greta", "Hugo", "Iris",
    "Jonas", "Kira", "Luis", "Maya", "Nils", "Olga", "Pavel", "Quinn", "Rosa",
    "Sven", "Tessa", "Umar", "Vera", "Wanda", "Xiu", "Yara", "Zane",
]
LAST = [
    "Silva", "Kowalski", "Nakamura", "Okafor", "Berg", "Haddad", "Novak",
    "Rossi", "Dubois", "Andersen", "Fischer", "Moreau", "Petrov", "Sato",
    "Vargas", "Weber",
]


def main() -> None:
    rng = random.Random(20240115)

    # ---------------------------------------------------------------- customers
    customers = []
    for index in range(1, 61):
        first = rng.choice(FIRST)
        last = rng.choice(LAST)
        name = f"{first} {last}"
        if index in (7, 23):
            name = f"  {first}   {last} "  # messy whitespace for normalize_text()
        email = f"{first.lower()}.{last.lower()}{index}@example.com"
        if index == 11:
            email = "not-an-email"          # is_email() catches this
        if index == 29:
            email = "broken@@example.com"   # ... and this
        if index == 41:
            email = ""                      # persisted as NULL
        signed = date(2023, 1, 1) + timedelta(days=rng.randrange(0, 700))
        customers.append(
            {
                "customer_id": f"C{index:04d}",
                "full_name": name,
                "email": email,
                "country": rng.choice(COUNTRIES),
                "signed_up_at": signed.isoformat(),
                "marketing_opt_in": "true" if rng.random() < 0.62 else "false",
            }
        )
    # a duplicated customer id, planted for the `unique` rule
    customers.append(dict(customers[4]))

    # ----------------------------------------------------------------- products
    products = []
    for index in range(1, 31):
        category = rng.choice(CATEGORIES)
        price = round(rng.choice([19, 29, 49, 99, 149, 249, 399, 899]) * (1 + rng.random() / 8), 2)
        products.append(
            {
                "product_id": f"P{index:04d}",
                "sku": f"{category[:3].upper()}-{index:04d}",
                "name": f"{category.title()} plan {index}",
                "category": category,
                "unit_price": f"{price:.2f}",
                "active": "true" if rng.random() < 0.85 else "false",
            }
        )

    # ------------------------------------------------------------------- orders
    order_rows = []
    order_id = 10_000
    for month, days in ((1, 31), (2, 29)):
        rows = []
        for _ in range(120):
            order_id += 1
            customer = rng.choice(customers)
            product = rng.choice(products)
            quantity = rng.randrange(1, 6)
            unit = float(product["unit_price"])
            amount = round(unit * quantity * (1 - rng.choice([0, 0, 0, 0.05, 0.1])), 2)
            created = date(2024, month, rng.randrange(1, days + 1))
            rows.append(
                {
                    "order_id": f"O{order_id}",
                    "customer_id": customer["customer_id"],
                    "product_id": product["product_id"],
                    "quantity": str(quantity),
                    "amount": f"{amount:.2f}",
                    "status": rng.choice(STATUSES),
                    "created_at": created.isoformat(),
                    "channel": rng.choice(CHANNELS),
                    "customer_email": customer["email"],
                }
            )

        # defects the rules file asserts on
        rows[3]["amount"] = "-149.00"          # negative amount
        rows[17]["customer_id"] = ""           # NULL reference
        rows[42]["status"] = "COMPLETED"       # outside accepted_values
        rows[55]["amount"] = "-9.99"
        if month == 2:
            rows[9]["order_id"] = rows[8]["order_id"]          # duplicate key
            rows[9]["customer_email"] = "n/a"                   # invalid email

        order_rows.append((month, rows))

    # -------------------------------------------------------------------- write
    (DATA / "orders").mkdir(parents=True, exist_ok=True)

    write_csv(DATA / "customers.csv", customers)
    write_csv(DATA / "products.csv", products)
    for month, rows in order_rows:
        write_csv(DATA / "orders" / f"orders_2024_{month:02d}.csv", rows)

    print(f"wrote {len(customers)} customers, {len(products)} products, "
          f"{sum(len(r) for _, r in order_rows)} orders")


def write_csv(path: Path, rows: list[dict[str, str]]) -> None:
    with path.open("w", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=list(rows[0].keys()))
        writer.writeheader()
        writer.writerows(rows)


if __name__ == "__main__":
    main()
