def total(items, discount=0):
    """What a basket of (price, quantity) pairs costs."""
    subtotal = sum(price for price, _ in items)
    return subtotal - discount
