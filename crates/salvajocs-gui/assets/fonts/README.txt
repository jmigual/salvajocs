AtkinsonHyperlegibleNext.ttf is the variable font (wght 200 to 800) from Google Fonts, under the
SIL Open Font License 1.1 (see OFL.txt):

    https://raw.githubusercontent.com/google/fonts/66eef70728ca29f28c31cdc5e7947f2c2de73e8d/ofl/atkinsonhyperlegiblenext/AtkinsonHyperlegibleNext%5Bwght%5D.ttf
    sha256 5a455d1cfa099b601ab70751bb9673e8fe1854dc4500c80e1a220d0d75e31745

salvajocs-icons.ttf is a subset of the Phosphor Icons regular font (MIT, see
Phosphor-LICENSE.txt), holding only the icons the app draws:

    https://raw.githubusercontent.com/phosphor-icons/web/v2.1.2/src/regular/Phosphor.ttf
    sha256 06b91e022b7ee899a63efced879392a74f0bacbda54e4467e9f663220d173a10

Codepoints, as named in phosphor-icons/web:

    E248 floppy-disk          E1A0 clock-counter-clockwise   E26E game-controller
    E272 gear-six             E30C magnifying-glass          E256 folder-open
    E3D4 plus                 E4A6 trash                     E3B4 pencil-simple
    E4E0 warning              E4F8 x-circle                  E184 check-circle
    E21C equals               EB02 linux-logo                E692 windows-logo
    EAD4 steam-logo           E06C arrow-right               E5DE arrow-square-out
    E030 arrow-circle-up      E40C shield-check              E308 lock-simple
    E30A lock-simple-open

Regenerate with:

    pip install fonttools
    pyftsubset Phosphor.ttf \
        --unicodes=E248,E1A0,E26E,E272,E30C,E256,E3D4,E4A6,E3B4,E4E0,E4F8,E184,E21C,EB02,E692,E06C,E5DE,E030,E40C,E308,E30A,EAD4 \
        --no-hinting --desubroutinize --layout-features='' \
        --output-file=salvajocs-icons.ttf

The binary also embeds egui's default fonts Ubuntu Light and Hack, from the
epaint_default_fonts 0.37.0 crate (https://github.com/emilk/egui, crates/epaint_default_fonts).
Their license texts are copied from that crate's fonts/ folder:

    Ubuntu-font-LICENSE.txt   fonts/UFL.txt, the Ubuntu Font Licence 1.0, for Ubuntu-Light.ttf
    Hack-LICENSE.txt          fonts/Hack-Regular.txt, MIT and the Bitstream Vera License, for
                              Hack-Regular.ttf
