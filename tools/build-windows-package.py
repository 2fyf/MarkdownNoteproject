"""Build a minimal Windows installer and portable archive from a compiled EXE."""
from pathlib import Path
from zipfile import ZipFile, ZIP_DEFLATED
import argparse
import hashlib
import json
import shutil
import subprocess


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    project = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--exe', required=True, type=Path)
    parser.add_argument('--nsis', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--date', required=True, help='Release date, YYYY-MM-DD')
    args = parser.parse_args()
    exe, compiler, output = args.exe.resolve(), args.nsis.resolve(), args.output.resolve()
    # A sibling output directory keeps source/runtime directories out of cleanup.
    if not output.is_relative_to(project.parent) or output == project.parent or output.is_relative_to(project):
        raise SystemExit('Output must be a separate directory inside the project workspace.')
    if output.exists() and any(output.iterdir()):
        raise SystemExit('Output is not empty; use a new release directory. No files overwritten.')
    if not exe.is_file() or not compiler.is_file():
        raise SystemExit('Compiled executable or NSIS compiler not found.')
    with exe.open('rb') as f:
        if f.read(2) != b'MZ':
            raise SystemExit('Input is not a Windows executable.')

    def git(*parts):
        return subprocess.check_output(['git', '-C', str(project), *parts])

    if git('status', '--porcelain').strip():
        raise SystemExit('Commit source changes first so the source archive matches a fixed revision.')
    commit = git('rev-parse', 'HEAD').decode('ascii').strip()
    config = json.loads((project / 'src-tauri/tauri.conf.json').read_text(encoding='utf-8'))
    version = config['version']
    if config['productName'] != 'MarkdownNote' or config['identifier'] != 'com.course.markdownnote':
        raise SystemExit('Unexpected product identity.')

    output.mkdir(parents=True, exist_ok=True)
    payload = output / f'MarkdownNote-{version}'
    legal = payload / '文档/开源许可'
    legal.mkdir(parents=True)
    shutil.copy2(exe, payload / 'MarkdownNote.exe')
    (payload / '.markdownnote-app').write_text('com.course.markdownnote\n', encoding='utf-8')
    shutil.copy2(project / 'tools/使用说明.md', payload / '使用说明.md')
    for name in ['LICENSE', 'NOTICE']:
        shutil.copy2(project / name, legal / name)
    shutil.copy2(project / '文档/开源许可/第三方许可.md', legal / '第三方许可.md')
    files = sorted(p for p in payload.rglob('*') if p.is_file())
    manifest = [{'path': p.relative_to(payload).as_posix(), 'bytes': p.stat().st_size, 'sha256': digest(p)} for p in files]
    (payload / '程序文件清单.json').write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    files = sorted(p for p in payload.rglob('*') if p.is_file())
    expected = {'.markdownnote-app', 'MarkdownNote.exe', '使用说明.md', '程序文件清单.json',
                '文档/开源许可/LICENSE', '文档/开源许可/NOTICE', '文档/开源许可/第三方许可.md'}
    if {p.relative_to(payload).as_posix() for p in files} != expected:
        raise SystemExit('Unexpected installer payload.')

    portable = output / f'MarkdownNote-{version}-portable.zip'
    with ZipFile(portable, 'w', ZIP_DEFLATED) as archive:
        for p in files:
            archive.write(p, f'MarkdownNote-{version}/' + p.relative_to(payload).as_posix())
    source = output / f'MarkdownNote-{version}-source.zip'
    subprocess.run(['git', '-C', str(project), 'archive', '--format=zip',
                    '--prefix=MarkdownNote-source/', '--output=' + str(source), commit], check=True)

    install, uninstall = [], []
    directories = set()
    for p in files:
        relative = str(p.relative_to(payload)).replace('/', '\\')
        parent = str(p.relative_to(payload).parent).replace('/', '\\')
        install.append('SetOutPath "$INSTDIR' + ('\\' + parent if parent != '.' else '') + '"')
        install.append('File "' + str(p) + '"')
        uninstall.append('Delete "$INSTDIR\\' + relative + '"')
        directories.update(str(d).replace('/', '\\') for d in p.relative_to(payload).parents if str(d) != '.')
    remove_dirs = ['RMDir "$INSTDIR\\' + name + '"' for name in sorted(directories, key=len, reverse=True)]
    installer = output / f'MarkdownNote-{version}-setup.exe'
    template = (project / 'tools/windows-installer.nsi').read_text(encoding='utf-8-sig')
    replacements = {'@OUTFILE@': str(installer), '@VERSION@': version, '@ICON@': str(project / 'src-tauri/icons/markdownnote.ico'),
                    '@INSTALL@': '\n'.join(install), '@UNINSTALL@': '\n'.join(uninstall), '@RMDIRS@': '\n'.join(remove_dirs)}
    for key, value in replacements.items():
        if key not in template:
            raise SystemExit('Missing NSIS substitution: ' + key)
        template = template.replace(key, value)
    script = output / '安装包.nsi'
    script.write_text(template, encoding='utf-8-sig')
    with (output / '安装包构建.log').open('w', encoding='utf-8') as log:
        result = subprocess.run([str(compiler), '/V2', str(script)], stdout=log, stderr=subprocess.STDOUT)
    if result.returncode:
        raise SystemExit('NSIS build failed; see the build log. Existing releases are unchanged.')
    for path in [portable, source]:
        with ZipFile(path) as archive:
            if archive.testzip() is not None:
                raise SystemExit('Archive CRC check failed: ' + path.name)
    with ZipFile(source) as archive:
        entries = [p for p in archive.namelist() if not p.endswith('/')]
    record = {'date': args.date, 'version': version, 'sourceCommit': commit,
              'executableSHA256': digest(exe), 'payloadFileCount': len(files), 'sourceFileCount': len(entries),
              'artifacts': [{'file': p.name, 'bytes': p.stat().st_size, 'sha256': digest(p)} for p in [portable, installer, source]],
              'personalDataIncluded': False, 'installerRuntimeValidation': 'pending',
              'registrationScope': 'Installer-adjacent INI and target marker; no global registry registration',
              'webview2': 'Requires an existing WebView2 Runtime; not bundled or auto-installed'}
    (output / '交付清单.json').write_text(json.dumps(record, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(record, ensure_ascii=False))


if __name__ == '__main__':
    main()
