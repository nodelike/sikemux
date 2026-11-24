import type { BillingInterval, BillingProduct } from "@sikemux/protocol";

import { priceOf } from "./money.ts";
import { Seg } from "./ui.tsx";

/** Monthly or yearly. Given a product, the yearly side says how many of its months come free. */
export function BilledSwitch({
  interval,
  onChange,
  saving,
}: {
  interval: BillingInterval;
  onChange: (interval: BillingInterval) => void;
  saving?: BillingProduct;
}) {
  const month = priceOf(saving, "month");
  const year = priceOf(saving, "year");
  const monthsFree =
    month && year ? 12 - Math.round(year.amount / month.amount) : 0;
  return (
    <Seg
      label="Billed"
      value={interval}
      onChange={onChange}
      options={[
        { value: "month", name: "Monthly" },
        {
          value: "year",
          name: (
            <>
              Yearly
              {monthsFree > 0 ? (
                <em className="save">{monthsFree} months free</em>
              ) : null}
            </>
          ),
        },
      ]}
    />
  );
}
