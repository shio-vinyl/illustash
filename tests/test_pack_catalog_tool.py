"""tools/pack_catalog.py（#602）：条目从两个组成部分的包文件读字段、组目录时序号加一、发布时拒绝应用会跳过的条目、核对签名、打离线包。

夹具在 tests/fixtures/pack_catalog/（全部合成）。签名的完整核对要装 cryptography；没装时只核对密钥编号，测试照样跑。
"""
import json
from pathlib import Path
import tempfile
import unittest
import zipfile

from tools import pack_catalog
from tests.pack_shards_fixture import shard_fixture

FIXTURES = Path(__file__).resolve().parent / 'fixtures' / 'pack_catalog'
PLACEHOLDER = ('# 专签数据包目录的公钥（#602），Tauri 格式：`cargo tauri signer generate` 打印的那一行。\n'
               '# 占位：还没有配置。用户用 `python3 tools/pack_catalog.py keygen` 生成密钥对后，把公钥写在下面一行（不带 `#`）。\n'
               '# 只有说明行时，应用不读包目录，只用内置的两个包。私钥只在用户自己的电脑上，不进仓库。\n')


class PackCatalogToolTest(unittest.TestCase):
    def test_character_index_version_has_its_own_signed_file(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'character_catalog.json'
            path.write_text(json.dumps({'format': pack_catalog.CHARACTER_FORMAT, 'revision': 2}))
            item = pack_catalog.character_entry(path, '0.4.2', 'fixture')
            manifest = pack_catalog.build([item])
            self.assertEqual(item['version'], 'catalog-2')
            self.assertNotIn('components', item)
            path.write_text(path.read_text() + '\n')
            with self.assertRaisesRegex(pack_catalog.CatalogError, '不能替换'):
                pack_catalog.build([pack_catalog.character_entry(path, '0.4.2', 'fixture')], manifest)

    def test_entry_reads_both_components_from_their_packages(self):
        e = pack_catalog.entry(version='1.0', revision=3, wd=FIXTURES / 'wd-extension-v3.bin', ccip=FIXTURES / 'ccip-longtail-v2.zip',
                               min_app='0.3.0')
        self.assertEqual((e['kind'], e['version'], e['license']), ('character-pack', '1.0', 'Apache-2.0 AND MIT'))
        self.assertNotIn('stage', e, '不写就是正式版')
        wd, ccip = e['components']['wd'], e['components']['ccip']
        self.assertEqual((wd['format'], wd['heads'], wd['candidate_only'], wd['license']),
                         ('illustash-wd-extension/2', 3, 1, 'Apache-2.0'))
        self.assertEqual(wd['file']['url'], pack_catalog.RELEASE_PREFIX + 'character-pack-1.0/wd-extension-v3.bin')
        self.assertEqual(set(wd['base']), {'wd_model_sha256', 'wd_tags_sha256'})
        self.assertEqual((ccip['format'], ccip['heads'], ccip['license']), ('illustash-557-ccip-offline/1', 2, 'MIT'))
        self.assertEqual(set(ccip['base']), {'ccip_model_sha256'})
        # 和上一版相同的组成部分指向原来的 Release，可以换发布用的名字。
        e = pack_catalog.entry(version='0.2', stage='preview', revision=2, wd=FIXTURES / 'wd-extension-v2.bin',
                               ccip=FIXTURES / 'ccip-longtail-v1.zip', ccip_tag='ccip-longtail-v1', ccip_name='A-longtail-p75.zip',
                               min_app='0.3.0')
        self.assertEqual(e['stage'], 'preview')
        self.assertTrue(e['components']['wd']['file']['url'].endswith('/character-pack-0.2/wd-extension-v2.bin'))
        self.assertTrue(e['components']['ccip']['file']['url'].endswith('/ccip-longtail-v1/A-longtail-p75.zip'))
        with self.assertRaises(pack_catalog.CatalogError):
            pack_catalog.build([{**e, 'stage': 'rc'}])
        with self.assertRaises(pack_catalog.CatalogError):
            pack_catalog.entry(version='0.2', revision=2, wd=FIXTURES / 'wd-extension-v2.bin',
                               ccip=FIXTURES / 'ccip-longtail-v1.zip', min_app='0.3')
        with self.assertRaises(pack_catalog.CatalogError):
            pack_catalog.entry(version='0.2', revision=2, wd=FIXTURES / 'wd-extension-v2.bin',
                               ccip=FIXTURES / 'ccip-longtail-v1.zip', min_app='0.3.0', note_zh='只有中文')
        # 组成部分放反了：包文件读不出来。
        with self.assertRaises(Exception):
            pack_catalog.entry(version='0.2', revision=2, wd=FIXTURES / 'ccip-longtail-v1.zip',
                               ccip=FIXTURES / 'wd-extension-v2.bin', min_app='0.3.0')

    def test_build_merges_by_version_and_bumps_the_serial(self):
        entry = lambda version, revision, wd, ccip: pack_catalog.entry(version=version, revision=revision, wd=FIXTURES / wd,
                                                                       ccip=FIXTURES / ccip, min_app='0.3.0')
        v2 = entry('0.2', 2, 'wd-extension-v2.bin', 'ccip-longtail-v1.zip')
        first = pack_catalog.build([v2])
        self.assertEqual((first['serial'], [p['version'] for p in first['packs']]), (1, ['0.2']))
        # 两个版本共用一个 CCIP 文件可以。
        v3 = entry('1.0', 3, 'wd-extension-v3.bin', 'ccip-longtail-v1.zip')
        second = pack_catalog.build([v3, {**v2, 'license': 'Apache-2.0'}], first)
        self.assertEqual(second['serial'], 2)
        self.assertEqual([(p['version'], p['license']) for p in second['packs']],
                         [('0.2', 'Apache-2.0'), ('1.0', 'Apache-2.0 AND MIT')])
        # 同一个修订号不能出现两次。
        with self.assertRaises(pack_catalog.CatalogError):
            pack_catalog.build([{**v3, 'version': '1.0b'}], second)
        # 发布时拒绝应用会跳过的条目；测试夹具用 strict=False 才能写进去。
        future = {**v2, 'version': 'future-v1', 'kind': 'future-pack', 'revision': 9}
        with self.assertRaises(pack_catalog.CatalogError):
            pack_catalog.build([future])
        self.assertEqual(len(pack_catalog.build([future], strict=False)['packs']), 1)
        tiered_v1 = {**v2, 'version': '1.8', 'revision': 8,
                     'components': {**v2['components'], 'wd': {**v2['components']['wd'], 'candidate_only': 1}}}
        with self.assertRaises(pack_catalog.CatalogError):
            pack_catalog.build([tiered_v1])

    def test_shards_preserve_file_bytes_and_reuse_the_previous_release(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (first, second), (old, new) = shard_fixture(root)
            old_wd = old['packs'][0]['components']['wd']
            new_wd = new['packs'][1]['components']['wd']
            old_parts = pack_catalog.shard_files(old_wd['shards'])
            new_parts = pack_catalog.shard_files(new_wd['shards'])
            self.assertEqual(len(old_parts), 3)
            self.assertEqual(old_parts[:-1], new_parts[:-1])
            self.assertNotEqual(old_parts[-1]['sha256'], new_parts[-1]['sha256'])
            self.assertEqual([path.name for path in (root / 'second').iterdir()], [new_parts[-1]['name']])
            self.assertEqual(new_wd['file']['sha256'], pack_catalog.sha256(second.read_bytes()))
            self.assertEqual(first.stat().st_size, second.stat().st_size)

            old_entry = old['packs'][0]
            new_entry = {**new['packs'][1], 'version': old_entry['version'], 'revision': old_entry['revision']}
            with self.assertRaisesRegex(pack_catalog.CatalogError, '不能替换'):
                pack_catalog.build([new_entry], old)

            new_wd['shards']['coefficients'][-1]['size'] += 1
            with self.assertRaisesRegex(pack_catalog.CatalogError, '分片不合法'):
                pack_catalog.check_catalog(new)

    def test_verify_checks_the_key_and_the_bytes(self):
        data = (FIXTURES / 'packs.json').read_bytes()
        a = (FIXTURES / 'key-a.pub').read_text()
        self.assertIn(pack_catalog.verify(data, (FIXTURES / 'packs.json.sig').read_text(), a), (True, None))
        self.assertIn(pack_catalog.verify(data, (FIXTURES / 'packs.json.tauri.sig').read_text(),
                                          (FIXTURES / 'tauri-key.pub').read_text()), (True, None))
        with self.assertRaisesRegex(pack_catalog.CatalogError, '不是用'):
            pack_catalog.verify(data, (FIXTURES / 'packs.json.sig-b').read_text(), a)
        try:
            import cryptography  # noqa: F401
        except ImportError:
            return
        with self.assertRaisesRegex(pack_catalog.CatalogError, '对不上'):
            pack_catalog.verify(data + b'\n', (FIXTURES / 'packs.json.sig').read_text(), a)

    def test_offline_bundle_has_catalog_signature_package_and_extras(self):
        with tempfile.TemporaryDirectory() as temp:
            output = Path(temp) / 'wd-extension-v2-offline.zip'
            notice = Path(temp) / 'NOTICE.txt'
            notice.write_text('synthetic notice\n')
            pack_catalog.offline(FIXTURES / 'packs.json', [FIXTURES / 'wd-extension-v2.bin', FIXTURES / 'ccip-longtail-v1.zip'],
                                 output, extra=[notice])
            with zipfile.ZipFile(output) as archive:
                self.assertEqual(archive.namelist(), ['packs.json', 'packs.json.sig', 'wd-extension-v2.bin',
                                                      'ccip-longtail-v1.zip', 'NOTICE.txt'])
                self.assertEqual(archive.read('packs.json'), (FIXTURES / 'packs.json').read_bytes())
            with self.assertRaises(pack_catalog.CatalogError):
                pack_catalog.offline(FIXTURES / 'packs.json', notice, output)

    def test_pubkey_file_placeholder_and_write(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'pack-key.pub'
            # 仓库里的 pack-key.pub 2026-11-18 起是正式公钥，这里用当初的占位写法。
            path.write_text(PLACEHOLDER, encoding='utf-8')
            self.assertIsNone(pack_catalog.configured_pubkey(path))
            key = (FIXTURES / 'key-a.pub').read_text().strip()
            pack_catalog.write_pubkey(key, path)
            self.assertEqual(pack_catalog.configured_pubkey(path), key)
            self.assertNotIn('占位', path.read_text(encoding='utf-8'))
            with self.assertRaises(pack_catalog.CatalogError):
                pack_catalog.write_pubkey('not a key', path)


if __name__ == '__main__':
    unittest.main()
