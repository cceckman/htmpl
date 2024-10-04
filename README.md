# htmpl: HTML templating in HTML

htmpl is a library for generating HTML files from HTML templates.

```html
<htmpl-query name="posts">SELECT title, text, draft FROM posts;</htmpl-query>
<htmpl-foreach query="posts">
    <h1>
        <htmpl-insert query="posts(title)"></htmpl-insert>
        <htmpl-if true="posts(draft)"> (Draft)</htmpl-if></h1>
    <p><htmpl-insert query="posts(text)"></htmpl-insert></p>
</htmpl-foreach>
```

See the [documentation](src/lib.md) (at [docs.rs](https://docs.rs/htmpl)) for more details.

> NOTE: htmpl is in the very early stages of development,
> and will have breaking changes in the future.

At the moment, it's mostly a proof-of-concept,
so [I](https://cceckman.com) can see how far this idea lets me go.

