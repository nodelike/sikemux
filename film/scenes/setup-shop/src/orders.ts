export type OrderStatus = "cart" | "placed" | "paid" | "shipped" | "refunded";

export interface Order {
  id: string;
  customerId: string;
  status: OrderStatus;
  totalCents: number;
}
