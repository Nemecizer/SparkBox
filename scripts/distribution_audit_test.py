import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('audit_distribution', Path(__file__).with_name('audit-distribution.py'))
audit_module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit_module)


class DistributionAuditTests(unittest.TestCase):
    def test_rejects_state_keys_private_identifiers_and_links_without_printing_values(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fixtures = {'inventory.json': '{}', '.env.local': 'value', 'signing.p12': 'value',
                        'example.txt': 'unique-private-marker', 'key.txt': '-----BEGIN ' + 'OPENSSH PRIVATE KEY-----\nblob',
                        'inline.rs': 'const KEY: &str = \"-----BEGIN ' + 'OPENSSH PRIVATE KEY-----\nblob',
                        'src/main.rs': 'fn main() {}'}
            for name, value in fixtures.items():
                p = root / name
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_text(value)
            (root / 'linked.txt').symlink_to(root / 'example.txt')
            result = audit_module.audit(root, [*fixtures, 'linked.txt'], ['unique-private-marker'])
            self.assertEqual({name for name, _ in result}, set(fixtures) - {'src/main.rs'} | {'linked.txt'})
            self.assertNotIn('unique-private-marker', str(result))

    def test_allows_labeled_fixture_addresses_and_protocol_constants(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            p = root / 'fixtures/inventory.json'
            p.parent.mkdir()
            p.write_text('{"note":"synthetic example","ip":"192.0.2.1","local":"127.0.0.1"}')
            self.assertEqual(audit_module.audit(root, ['fixtures/inventory.json']), [])


if __name__ == '__main__':
    unittest.main()
