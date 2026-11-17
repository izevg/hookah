import { greet, type User } from "@acme/lib";

export function welcome(name: string): string {
  const user: User = { name };
  return greet(user).toUpperCase();
}
