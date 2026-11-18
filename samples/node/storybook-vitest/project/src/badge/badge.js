export const badge = ({ count }) => {
  const el = document.createElement("span");
  el.className = "badge";
  el.textContent = count > 99 ? "99+" : String(count);
  return el;
};
