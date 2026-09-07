# Zaru

Culling de fotos RAW pelo teclado. Uma tecla por foto, avanço automático, sem
mouse e sem espera.

Depois de uma sessão em rajada sobram centenas ou milhares de CR3. Lightroom e
darktable levam segundos por foto para montar preview. Zaru não renderiza RAW:
serve o JPEG que a própria câmera já gravou dentro do arquivo.

Câmera alvo: Canon EOS R50 (CR3).

## Estado

**Completo.** Navegação, marcação, desfazer, coleções, gravação de XMP, mover
arquivos e o acabamento visual.

| Crate | O quê |
|---|---|
| `zaru-cr3` | Acha o JPEG embutido e a orientação. Zero dependências. |
| `zaru-xmp` | Lê, mescla e escreve `xmp:Rating` / `xmp:Label`. Zero dependências. |
| `zaru-core` | Sessão, marcas, desfazer, preferências, prefetch. Sem Tauri. |
| `src-tauri` | Janela, protocolo `zaru://` e os comandos. Só fiação. |
| `tools/ui-preview` | Renderiza a interface sem webview e tira screenshots. |

`zaru-core` não depende do Tauri de propósito: a máquina de desenvolvimento não
tem display, e sem isso a lógica das fases 1 e 2 ficaria sem teste nenhum.

```
cargo test --workspace --exclude zaru      # 56 testes, sem webview
cargo run --bin zaru-probe -- example_cr3.CR3 -o preview.jpg
cargo run --bin zaru-mark  -- IMG_4821.CR3 --rating 4 --label Green
cargo build --release --target x86_64-pc-windows-gnu
```

O app compila e linka para Windows a partir do Linux com `mingw-w64`, sem
`webkit2gtk` e sem CI. O único arquivo que precisa acompanhar o `zaru.exe` é a
`WebView2Loader.dll` que o build já deposita ao lado dele.

A máquina de desenvolvimento não tem display nem webview, então a interface não
roda nela. `node tools/ui-preview/preview.js` a desenha no Chromium do
Playwright com a ponte do Tauri dublada e escreve as telas em
`target/ui-preview/` — é o único jeito de olhar o resultado sem passar o
binário para outro computador.

## Teclas

| Tecla | Ação |
|---|---|
| `K` / `H` | foto anterior / próxima |
| `A` `R` `S` `T` `G` | 1 a 5 estrelas; a mesma tecla zera |
| `Espaço` | etiqueta verde |
| `Backspace` | rejeita e avança |
| `N` / `M` | nova coleção / mover para uma coleção |
| `Ctrl+Z` / `Ctrl+Shift+Z` | desfaz / refaz |
| `Ctrl+Enter` | aplicar |
| `O` / `C` / `?` | abrir pasta / ajustes / teclas |

Tudo é lido de `event.key`, nunca de `event.code`. Com Colemak-DH ativo no
sistema operacional, a tecla do `R` reporta `KeyS`, que é a posição física no
QWERTY; com o layout no firmware do teclado, as duas coincidem. `event.key`
funciona nos dois casos.

Estrela e etiqueta **não** avançam a foto — o usuário dá nota, olha de novo e
só então decide. Rejeitar avança, porque é a única decisão terminal.

## Como a navegação não espera

Nenhum byte de imagem passa pelo canal de comandos. Serializar um megabyte de
JPEG em base64 a cada tecla produz exatamente a latência que o app existe para
eliminar. As imagens saem por um esquema de URI próprio, `zaru://<índice>`, e o
front-end é uma `<img>`.

O prefetch tem duas camadas, porque o custo mudou de lugar: extrair não
decodifica nada, então o gargalo é o decode do JPEG de 6000×4000 pelo WebView.

- **Rust** mantém em memória as 5 fotos à frente e 3 atrás, lidas por um pool
  de threads. Isso paga o disco antecipadamente.
- **JavaScript** mantém um anel de 9 elementos `<img>` já decodificados via
  `img.decode()`. A tecla só troca qual deles está visível.

A barra inferior mostra a mediana e o p95 do tempo entre a tecla e a pintura,
medidos ao vivo. O critério de aceite da Fase 1 é um número, não uma impressão:
**12,3 ms** medidos numa pasta real, contra um orçamento de 16 ms — um frame a
60 Hz.

Por isso o plano de contingência não existe: estava previsto cair para a prévia
`PRVW` de 1620×1080 durante a navegação rápida se o decode da imagem inteira não
coubesse no orçamento. Coube. A resolução total fica o tempo todo.

## Onde fica a prévia num CR3

Um CR3 é um arquivo ISO-BMFF. A documentação informal costuma dizer que a
prévia grande está numa box `prvw`; no arquivo real ela não está. Medido em
`example_cr3.CR3`:

```
ftyp  @0        24 B
moov  @24       26720 B
  uuid 85c0b687…   CMT1 CMT2 CMT3 CMT4 THMB
  trak #1 → mdia/minf/stbl → co64 = 206848   stsz = 1150804   stsd = 6000×4000
  trak #2, trak #3
uuid  @26744    be7acfcb…
uuid  @92304    eaf42b5e…  → PRVW  1620×1080, 114 KB
mdat  @206832   28271484 B
```

| Fonte | Tamanho | Pixels |
|---|---|---|
| `THMB` | 7 KB | thumbnail |
| **sample do trak #1** | **1.15 MB** | **6000×4000** |
| `PRVW` | 114 KB | 1620×1080 |

Zaru serve o sample do trak #1: resolução total, qualidade 82, sRGB. Localizar
custa um punhado de leituras no `moov`; entregar custa um `seek` e um `read`.
**Não há decodificação de imagem no lado Rust** — quem decodifica é o WebView.

Dois detalhes que o container esconde:

- **O JPEG do trak #1 não tem segmento Exif.** A orientação vem da box `CMT1`,
  que é uma IFD0 TIFF pura (tag `0x0112`). Sem ler dali, toda foto em retrato
  aparece deitada.
- **Nem todo CR3 põe um JPEG no trak #1.** A varredura de tracks é protegida
  pelo magic `FF D8 FF` e cai para a box `PRVW` quando nenhum track serve.

## Coleções

Uma coleção é uma subpasta da pasta de trabalho. Sem banco, sem catálogo, sem
estado escondido.

```
2026-09-05-interlagos/
├── IMG_4820.CR3
├── IMG_4820.xmp
├── porsche/
├── ferrari/
└── descarte/
```

`N` cria — modal longo, campo de texto, e enquanto está aberto **todas** as
teclas pertencem ao campo; sem isso o usuário entra no modo sem querer e as
próximas cinco teclas viram nome de pasta. `M` atribui — modal curto, lista
numerada, uma tecla de `1` a `9` e fecha. Daí o limite de nove coleções: uma
décima não teria tecla. `0` tira a foto da coleção. Uma foto pertence a no
máximo uma.

Nada disso toca o disco durante a triagem. Mover arquivo no meio do laço
invalidaria o índice da lista, que é todo o senso de "onde estou" do app, e
faria "próxima foto" significar coisa diferente a cada tecla. As coleções vivem
em memória até o passo de Aplicar; abandonar a sessão não deixa pasta vazia
para trás.

## Aplicar

`Ctrl+Enter` mostra o que vai acontecer antes de acontecer:

```
487  fotos avaliadas
312  vão receber .xmp
 84  vão para porsche/ (168 arquivos)
 61  vão para ferrari/
103  rejeitadas — permanecem onde estão
753  sem marcação, ficam como estão
```

Mover arquivo é a única coisa irreversível que o Zaru faz, então a prévia não é
enfeite. Colisões de nome são detectadas **antes** de qualquer escrita, e
enquanto existir uma o Aplicar se recusa a começar — nada pela metade.

Depois de confirmar: os `.xmp` primeiro, os moves depois. Nessa ordem sempre,
senão o sidecar recém-escrito ficaria para trás enquanto a foto vai para a
coleção. Cada CR3 leva junto tudo que compartilha o *stem* — o `.xmp` e o
`.JPG` irmão, se a câmera estava em RAW+JPEG. O casamento é pelo prefixo
`nome.`, e não pelo *stem* que o sistema calcula, porque o sidecar do darktable
é `IMG_4821.CR3.xmp`: o *stem* dele é `IMG_4821.CR3`, não `IMG_4821`. O ponto no
prefixo é o que mantém `IMG_48210.CR3` de fora.

Se um move falhar — permissão, disco cheio — a execução para ali, diz em qual
arquivo, e tudo depois dele fica intacto.

## Direção visual

Neobrutalismo, com uma restrição vinda do assunto que manda em tudo o que vem
depois dela.

O app existe para julgar exposição, foco e cor. **A moldura em volta da foto não
pode mentir sobre nenhuma das três.** Cinza neutro é a superfície padrão de
avaliação de imagem exatamente por isso: qualquer tinta no fundo desloca a
percepção de branco, e qualquer coisa clara em volta faz a sombra parecer mais
fechada do que é.

Então o visor é cinza neutro puro e nada mais. O neobrutalismo mora na *chrome*
— barras, chips, painéis — encostada nas bordas da janela, e nunca entra na
área da imagem. A foto não ganha borda, sombra nem moldura: ela é o conteúdo, o
resto é o aparelho.

Seis cores, três delas semânticas:

```
--visor   #1C1C1C     --green   #00A651
--chrome  #E8E6E1     --reject  #FF3B2F
--ink     #000000     --star    #FFD400
```

`--ink` é `#000000` de verdade. Neobrutalismo com preto amaciado perde o ponto
inteiro — a dureza é a linguagem. Borda de 3px, sombra deslocada sem desfoque
(`5px 5px 0`), `border-radius: 0` em tudo.

Uma família só, **Archivo** variável, empacotada localmente porque o app é
offline. O eixo de largura faz a hierarquia que normalmente pediria uma segunda
fonte: contador em 125% de largura e peso 800, interface em 100%, informação
secundária em 87%. Números sempre em `tabular-nums`, senão o contador dança a
cada foto e o olho persegue o movimento.

O chip de coleção é colorido por hash do nome, com borda e texto pretos como
todo o resto. O hash passa por uma avalanche antes de virar matiz: multiplicar
e somar preserva vizinhança, e punha "porsche" e "ferrari" a sete graus de
distância — o mesmo rosa duas vezes.

A nota aparece como cinco células duras em vez de estrelas — no peso de borda
do resto da interface, uma fileira de quadrados preenchidos se lê como nota num
relance e fala o mesmo vocabulário que todo o resto da barra. Rejeição não
convive com a nota: as duas disputam o mesmo campo XMP, então o medidor **sai**
e dá lugar ao chip vermelho.

Movimento: praticamente nenhum. A troca de foto é instantânea, sem fade e sem
slide — transição ali é latência disfarçada de refinamento. O único movimento
permitido é a confirmação: a nota ou a etiqueta pisca uma vez, em 110 ms, ao ser
aplicada. `prefers-reduced-motion` desliga isso.

## Sidecar XMP

`xmp:Rating` acumula dois papéis: `-1` é rejeitado, `0` é sem nota, `1..=5` são
estrelas. Rejeição e nota disputam o mesmo campo — dar estrela numa foto
rejeitada tira a rejeição, e isso é correto, não é limitação. `xmp:Label` guarda
a cor, uma só por foto.

A mesclagem é cirúrgica: reescreve essas duas propriedades e deixa o resto do
arquivo byte a byte como estava, porque o sidecar pode já conter revelação,
palavras-chave ou recorte. As duas serializações são tratadas — Lightroom
escreve as propriedades como atributos de `rdf:Description`, darktable como
elementos filhos.

Os dois programas também discordam do nome do arquivo, então a escolha é
explícita — em **Ajustes** (`C`) no app, ou por argumento no `zaru-mark`:

| Ajuste | Resultado |
|---|---|
| Lightroom (padrão) | `IMG_4821.xmp` |
| darktable (`--darktable`) | `IMG_4821.CR3.xmp` |
| Os dois | os dois nomes, mesmo conteúdo |

A opção "os dois" existe porque nenhum dos dois programas lê o nome do outro de
forma confiável, e adivinhar no código seria pior do que perguntar. Custa um
arquivo pequeno a mais por foto.

## O que Zaru não faz

Não apaga arquivo, nunca. Rejeitar é uma anotação no XMP e nada além disso — o
que fazer com as rejeitadas depois é decisão sua, com as suas ferramentas.
Também não revela RAW, não edita, não mantém catálogo e não importa cartão.

## Licença

MIT ou Apache-2.0.

A fonte Archivo, em `ui/fonts/`, é de Omnibus-Type e está sob a SIL Open Font
License 1.1 — o texto da licença acompanha os arquivos em `ui/fonts/OFL.txt`.
