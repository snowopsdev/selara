"""Extract an untrusted release archive under one expected top-level directory."""
import pathlib
import sys
import tarfile

archive, destination, expected = sys.argv[1:]
with tarfile.open(archive) as source:
    total = 0
    for count, member in enumerate(source):
        path = pathlib.PurePosixPath(member.name)
        if count > 100_000 or path.is_absolute() or '..' in path.parts or not path.parts or path.parts[0] != expected:
            raise ValueError('Unexpected release archive path')
        if not (member.isfile() or member.isdir() or member.issym()):
            raise ValueError('Unsupported release archive entry')
        total += member.size
        if total > 4 * 1024**3:
            raise ValueError('Release archive is too large')
        if member.issym():
            target = pathlib.PurePosixPath(member.linkname)
            if target.is_absolute():
                raise ValueError('Absolute archive symlink')
            parts = list(path.parent.parts)
            for part in target.parts:
                if part == '..':
                    if len(parts) <= 1:
                        raise ValueError('Archive symlink escapes the application')
                    parts.pop()
                elif part != '.':
                    parts.append(part)
        source.extract(member, destination, filter='data')
