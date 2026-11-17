const { addItem, totalCents, isEmpty } = require('../src/cart');

test('adds up the prices', () => {
  const cart = addItem(addItem([], 'tea', 350), 'cake', 475);
  expect(totalCents(cart)).toBe(825);
});

test('an empty cart', () => {
  expect(isEmpty([])).toBe(true);
  expect(isEmpty(addItem([], 'tea', 350))).toBe(false);
});
