"""Points links in files included from the repository root at their site pages or GitHub."""

import re

from markdown import Extension
from markdown.preprocessors import Preprocessor

REPO = "https://github.com/sayef/strato/blob/main/"
PAGES = {"CHANGELOG.md": "changelog.md", "CONTRIBUTING.md": "contributing.md"}
LINK = re.compile(r"\]\((CHANGELOG\.md|CONTRIBUTING\.md|SECURITY\.md|CODE_OF_CONDUCT\.md|LICENSE)(#[^)]*)?\)")


class RepoLinks(Preprocessor):
    def run(self, lines):
        def target(match):
            name, anchor = match.group(1), match.group(2) or ""
            return f"]({PAGES.get(name, REPO + name)}{anchor})"

        return [LINK.sub(target, line) for line in lines]


class RepoLinksExtension(Extension):
    def extendMarkdown(self, md):
        md.preprocessors.register(RepoLinks(md), "strato_repo_links", 20)


def on_config(config):
    config.markdown_extensions.append(RepoLinksExtension())
    return config
