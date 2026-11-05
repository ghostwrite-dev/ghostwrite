import unittest

from cart import total


class TotalTest(unittest.TestCase):
    def test_counts_each_item_bought(self):
        self.assertEqual(total([(3, 2), (5, 1)]), 11)

    def test_takes_off_the_discount(self):
        self.assertEqual(total([(10, 1)], discount=2), 8)


if __name__ == "__main__":
    unittest.main()
