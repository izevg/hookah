export const label = (count) => (count > 99 ? "99+" : String(count));

export const plural = (n, word) => (n === 1 ? word : `${word}s`);
