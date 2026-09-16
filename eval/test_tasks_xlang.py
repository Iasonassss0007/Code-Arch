"""Hermetic checks for the paperless cross-language task builder."""
import build_tasks_xlang as X

URLS = '''
from documents.views import TagViewSet, BulkEditView, StatsView
from rest_framework.routers import DefaultRouter
router = DefaultRouter()
router.register(r"tags", TagViewSet)
router.register(r"documents", TagViewSet)
urlpatterns = [
    re_path(r"^api/", include([
        re_path("^documents/", include([
            re_path("^bulk_edit/", BulkEditView.as_view(), name="bulk_edit"),
        ])),
        path("stats/", StatsView.as_view()),
        *router.urls,
    ])),
]
'''


def test_routes_resolve_in_django_order_with_router_last():
    table = X.routes(URLS)
    assert X.resolve('api/documents/bulk_edit/', table) == ('BulkEditView', 'src/documents/views.py')
    assert X.resolve('api/documents/1/', table)[0] == 'TagViewSet'   # router catches the rest
    assert X.resolve('api/stats/', table)[0] == 'StatsView'
    assert X.resolve('api/tags/1/notes/', table)[0] == 'TagViewSet'
    assert X.resolve('api/nope/', table) is None


def test_endpoints_harvest_resource_services_and_templates():
    service = '''export class TagService extends AbstractNameFilterService<Tag> {
      constructor() { super(); this.resourceName = 'tags' }
      bulk() { return this.http.post(this.getResourceUrl(null, 'bulk_edit'), {}) }
      notes(id) { return this.http.get(this.getResourceUrl(id, 'notes')) }
    }'''
    assert X.endpoints(service) == {'api/tags/', 'api/tags/1/', 'api/tags/bulk_edit/', 'api/tags/1/notes/'}
    widget = "this.http.get(`${environment.apiBaseUrl}statistics/`)"
    assert X.endpoints(widget) == {'api/statistics/'}
    profile = "private endpoint = 'profile'\n get() { return this.http.get(`${environment.apiBaseUrl}${this.endpoint}/totp/`) }"
    assert X.endpoints(profile) == {'api/profile/totp/'}
    assert X.endpoints("const x = `hello ${name}`") == set()


def test_search_guess_ranks_files_by_class_name_overlap():
    files = {'a/tag.service.ts': ['this.resourceName = "tags"', 'class TagService'],
             'a/other.ts': ['nothing here'],
             'a/tag-list.component.ts': ['import { TagService }']}
    full, stripped = X.search_guess('TagViewSet', files, 2)
    assert 'a/other.ts' not in full and 'a/other.ts' not in stripped
    assert set(stripped) == {'a/tag.service.ts', 'a/tag-list.component.ts'}


def test_score_routes_parses_markdown_and_counts_fp_fn():
    import score_routes as S
    text = ("# Route callers\n"
            "\n## `TagViewSet` — `src/views.py`\n\nRoutes: `tags`\n\n"
            "- `ui/tag.service.ts`\n- `ui/extra.ts`\n"
            "\n## `DocView` — `src/views.py`\n\nRoutes: `documents`\n\n"
            "- `ui/doc.service.ts`\n")
    pred = S.parse(text)
    assert pred == {'TagViewSet': {'ui/tag.service.ts', 'ui/extra.ts'},
                    'DocView': {'ui/doc.service.ts'}}
    gold = {'TagViewSet': {'ui/tag.service.ts'}, 'DocView': {'ui/doc.service.ts'}}
    tp, fp, fn, rows = S.score(pred, gold)
    assert (tp, fp, fn) == (2, 1, 0)
    assert rows == [('FP', 'TagViewSet', 'ui/extra.ts')]
    tp2, fp2, fn2, rows2 = S.score({'TagViewSet': set()}, gold)
    assert (tp2, fp2, fn2) == (0, 0, 2)
    assert sorted(rows2) == [('FN', 'DocView', 'ui/doc.service.ts'),
                             ('FN', 'TagViewSet', 'ui/tag.service.ts')]
