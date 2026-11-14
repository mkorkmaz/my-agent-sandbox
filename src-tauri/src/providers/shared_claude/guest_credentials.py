import json
import os
import secrets
import stat
import sys

LIMIT = 1_048_576


def read_record(directory):
    try:
        descriptor = os.open('.credentials.json', os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory)
    except FileNotFoundError:
        return {}
    with os.fdopen(descriptor, 'rb') as source:
        metadata = os.fstat(source.fileno())
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.geteuid() or metadata.st_size > LIMIT:
            raise ValueError('Unsupported credential file')
        content = source.read(LIMIT + 1)
    if len(content) > LIMIT:
        raise ValueError('Credential file is too large')
    record = json.loads(content)
    if not isinstance(record, dict):
        raise ValueError('Unsupported credential record')
    return record


def update(path, operation, incoming=None):
    directory = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        record = read_record(directory)
        if operation == 'remove':
            if 'claudeAiOauth' not in record:
                return
            del record['claudeAiOauth']
        if operation == 'write':
            if not isinstance(incoming, dict) or set(incoming) != {'claudeAiOauth'} or not isinstance(incoming['claudeAiOauth'], dict) or incoming['claudeAiOauth'].get('refreshToken', 'missing') is not None:
                raise ValueError('The guest copy must not contain a refresh token')
            saved = record.get('claudeAiOauth')
            if isinstance(saved, dict) and all(saved.get(key) == value for key, value in incoming['claudeAiOauth'].items()):
                return
            record['claudeAiOauth'] = incoming['claudeAiOauth']
        if operation not in ('remove', 'write'):
            raise ValueError('Unsupported credential operation')
        if not record:
            os.unlink('.credentials.json', dir_fd=directory)
            os.fsync(directory)
            return
        content = json.dumps(record, separators=(',', ':')).encode('utf-8')
        if len(content) > LIMIT:
            raise ValueError('The combined credential record is too large')
        temporary = '.tugboat-credentials-' + secrets.token_hex(16)
        descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=directory)
        published = False
        try:
            with os.fdopen(descriptor, 'wb') as target:
                target.write(content)
                target.flush()
                os.fsync(target.fileno())
            os.replace(temporary, '.credentials.json', src_dir_fd=directory, dst_dir_fd=directory)
            published = True
            os.fsync(directory)
        finally:
            if not published:
                os.unlink(temporary, dir_fd=directory)
    finally:
        os.close(directory)


if __name__ == '__main__':
    try:
        if os.environ.get('CLAUDE_CONFIG_DIR') != '/brig/claude':
            raise ValueError('Unsupported Claude configuration directory')
        content = sys.stdin.buffer.read(LIMIT + 1) if sys.argv[1] == 'write' else None
        if content is not None and len(content) > LIMIT:
            raise ValueError('Credential input is too large')
        update('/brig/claude', sys.argv[1], json.loads(content) if content is not None else None)
    except (OSError, ValueError, KeyError, TypeError, IndexError):
        # Diagnostic output must not contain file contents or tokens.
        print('Could not update the managed Claude access copy.', file=sys.stderr)
        sys.exit(1)
