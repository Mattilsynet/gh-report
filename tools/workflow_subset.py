"""Reader for these workflow fixtures, NOT a general YAML parser.

Accepts two-space block mappings, step mappings, plain single-line strings,
literal | blocks, comments, and the exact tags: ["v*"] spelling. Quoting,
anchors, aliases, merge keys, other flow values, folded/chomped blocks,
multiline plain scalars and duplicate keys are unsupported (exit 2).
"""

import re
import sys


class Unavailable(ValueError):
    pass


def load(source):
    lines = source.splitlines()
    if "\t" in source or len(lines) > 2000:
        raise Unavailable("tabs or workflow exceeding 2000 lines")
    tokens = []
    i = 0
    while i < len(lines):
        line = lines[i]
        i += 1
        text = line.lstrip(" ")
        if not text or text.startswith("#"):
            continue
        indent = len(line) - len(text)
        if indent % 2:
            raise Unavailable("non-two-space indentation")
        item = text.startswith("- ")
        if item:
            text = text[2:]
        match = re.fullmatch(r"([A-Za-z_][A-Za-z_0-9-]*):(?: (.*))?", text)
        if not match:
            raise Unavailable("unsupported mapping line: " + text)
        key, value = match.groups()
        if value == "|":
            block = []
            base = indent + (4 if item else 2)
            while i < len(lines):
                following = lines[i]
                if following.strip() and len(following) - len(following.lstrip(" ")) < base:
                    break
                block.append(following[base:] if following.strip() else "")
                i += 1
            value = "\n".join(block).rstrip("\n") + "\n"
        elif value is not None:
            value = re.split(r"\s+#", value, maxsplit=1)[0].rstrip()
            if key == "tags" and value == '["v*"]':
                value = ["v*"]
            elif (not value or value[0] in "&*!>|'\"[{%@`" or ": " in value
                  or value in ("---", "...")):
                raise Unavailable("unsupported scalar: " + str(value))
        tokens.append((indent, item, key, value))

    def mapping(pos, level, initial=None):
        result = {} if initial is None else initial
        while pos < len(tokens) and tokens[pos][0] == level and not tokens[pos][1]:
            _, _, key, value = tokens[pos]
            if key in result:
                raise Unavailable("duplicate key: " + key)
            pos += 1
            if value is None:
                if pos < len(tokens) and tokens[pos][0] > level:
                    value, pos = node(pos, level + 2)
                else:
                    value = {}
            result[key] = value
        return result, pos

    def node(pos, level):
        if level > 32 or tokens[pos][0] != level:
            raise Unavailable("unsupported nesting/indentation")
        if not tokens[pos][1]:
            return mapping(pos, level)
        result = []
        while pos < len(tokens) and tokens[pos][0] == level and tokens[pos][1]:
            _, _, key, value = tokens[pos]
            if value is None:
                raise Unavailable("step must start with a scalar field")
            step, pos = mapping(pos + 1, level + 2, {key: value})
            result.append(step)
        return result, pos

    if not tokens:
        raise Unavailable("empty workflow")
    result, end = node(0, 0)
    if end != len(tokens) or not isinstance(result, dict):
        raise Unavailable("unsupported workflow structure")
    return result


def read_workflow(path):
    try:
        return load(path.read_text())
    except FileNotFoundError as error:
        print("MISSING WORKFLOW: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
    except (OSError, UnicodeError, Unavailable) as error:
        print("UNAVAILABLE: " + str(error), file=sys.stderr)
        raise SystemExit(2) from error


if __name__ == "__main__":
    try:
        load(sys.stdin.read())
    except Unavailable as error:
        print("UNAVAILABLE: " + str(error), file=sys.stderr)
        sys.exit(2)
