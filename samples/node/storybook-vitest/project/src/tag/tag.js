export const tag = ({ text }) => {
  const el = document.createElement("em");
  el.textContent = text.toUpperCase();
  return el;
};
