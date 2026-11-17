function addItem(cart, name, cents) {
  return [...cart, { name, cents }];
}

function totalCents(cart) {
  return cart.reduce((sum, item) => sum + item.cents, 0);
}

function isEmpty(cart) {
  return cart.length === 0;
}

module.exports = { addItem, totalCents, isEmpty };
