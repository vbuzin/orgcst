# Batch Emacs plus one pinned Org checkout. Distro Org is on the default
# load-path only after /opt/org/lisp, so it is not the oracle.
FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        emacs-nox \
        git \
        make \
    && rm -rf /var/lib/apt/lists/*

ARG ORG_COMMIT=10814296c77681fbbf708f0d898fa88cbaa7ccce

RUN git clone --filter=blob:none https://git.savannah.gnu.org/git/emacs/org-mode.git /opt/org \
    && git -C /opt/org checkout --detach "$ORG_COMMIT" \
    && make -C /opt/org autoloads \
    && test -f /opt/org/lisp/org-loaddefs.el \
    && test -f /opt/org/lisp/org-version.el

# Trailing colon keeps Emacs's default load-path after the pinned lisp.
ENV EMACSLOADPATH=/opt/org/lisp:

RUN emacs -Q --batch \
        --eval "(progn (require 'org) (require 'org-element) (princ (concat (org-version) \" \" (locate-library \"org\") \"\n\")))" \
        > /opt/org-pin.txt \
    && grep -q '/opt/org/lisp/' /opt/org-pin.txt
