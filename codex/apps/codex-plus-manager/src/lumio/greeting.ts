/** Turn `mary@example.com` into the home greeting name `Mary`. */
export function greetingNameFromEmail(email: string): string {
  const local = email.split("@")[0] ?? "";
  if (local === "") return "";
  return `${local.charAt(0).toUpperCase()}${local.slice(1)}`;
}
