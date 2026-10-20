# Машина шагов онбординга: какой шаг показан и когда переходить дальше.
# Чистая логика — без gi/GTK, чтобы проверяться против фейкового world
# (задача 6 даст настоящий, с реальным X11 за методами).
from targets import tray_area


class Machine:
    """Текущий шаг сценария и переходы между шагами.

    world — объект с методами icon(desktop_file), window(pattern, anchor),
    covered(rect) -> bool, screen() -> Rect. Сама Machine координат не
    считает — только решает, какая цель у текущего шага и пора ли дальше.
    """

    def __init__(self, steps, world):
        self._steps = list(steps)
        self._world = world
        self._index = 0
        self.done = False

    @property
    def current(self):
        return None if self.done else self._steps[self._index]

    @property
    def total(self):
        """Сколько всего шагов в сценарии — для подписи «Шаг N из M».
        Считается по сценарию, а не вписывается числом в рисование:
        число в коде расходится со сценарием при первом же добавленном
        шаге и врёт человеку про то, сколько ему осталось."""
        return len(self._steps)

    def _rect_for(self, target):
        """Прямоугольник на экране для данного Target прямо сейчас, или
        None, если его там нет. Общий поиск для цели стрелки (target) и
        условия перехода (wait_for) — разные Target, один и тот же способ
        их проверить."""
        if target.kind == 'none':
            return None
        if target.kind == 'desktop_icon':
            return self._world.icon(target.desktop_file)
        if target.kind == 'window':
            return self._world.window(target.name_pattern, anchor=target.anchor)
        if target.kind == 'tray':
            # Монитор целиком, не рабочая область: трей сидит в панели,
            # которую рабочая область как раз исключает (финальная
            # волна: иначе стрелка шага 6 указывала на пустоту над
            # значком, выше панели на её высоту).
            return tray_area(self._world.monitor())
        raise ValueError(f'неизвестный тип цели: {target.kind!r}')

    def _target_rect(self):
        """Цель текущего шага на экране прямо сейчас, или None, если её
        там нет (ещё не появилось окно/значок). Про перекрытие другим
        окном ничего не знает — этим занимается visible_target."""
        return self._rect_for(self.current.target)

    def _owner_rect(self, target):
        """Полный (не обрезанный anchor) прямоугольник окна цели — для
        covered(): у якорной части (anchor='right-bottom' и т.п.) нет
        своего места в списке окон, чтобы отличить «моё собственное
        окно» от «что-то чужое поверх», а у полного окна есть (финальная
        волна: World.window() отдавал уже обрезанный anchor'ом
        прямоугольник, он не совпадал ни с одним окном в списке — стрелка
        на шагах 4/5 считалась закрытой всегда, своим же окном)."""
        if target.kind == 'window':
            return self._world.window(target.name_pattern, anchor='whole')
        return None

    def _wait_rect(self):
        """Условие перехода текущего шага: wait_for, если шаг его задал,
        иначе тот же target (так вело себя tick() до разделения этих
        двух понятий). Цель стрелки (target) значка может стоять на
        экране всегда — ждать переход обязан не её, а wait_for (окно,
        которое правда появляется только по факту)."""
        step = self.current
        return self._rect_for(step.wait_for or step.target)

    def visible_target(self) -> 'Rect | None':
        """Куда рисовать стрелку. None, если цели нет вовсе (kind='none'),
        её не видно на экране или её закрывает другое окно — во всех
        трёх случаях стрелки не будет, а выноска остаётся. Безопасен и
        после done: шагов не осталось, значит и цели нет."""
        if self.done:
            return None
        rect = self._target_rect()
        if rect is None:
            return None
        if self._world.covered(rect, owner=self._owner_rect(self.current.target)):
            return None
        return rect

    def tick(self):
        """Вызывается по таймеру. Двигает шаг вперёд, если его
        advance_on == 'target' и условие перехода (wait_for, а если шаг
        его не задал — target) уже выполнено (закрыто оно другим окном
        или нет — неважно, перекрытие важно только для стрелки)."""
        if self.done:
            return
        step = self.current
        if step.advance_on == 'target' and self._wait_rect() is not None:
            self._advance()

    def press(self, button: str):
        """'next' — безусловный переход вперёд (человек имеет право
        пропустить шаг, даже если цель не появилась). 'close' — конец
        сценария с любого шага."""
        if button == 'close':
            self.done = True
            return
        if button == 'next':
            self._advance()
            return
        raise ValueError(f'неизвестная кнопка: {button!r}')

    def _advance(self):
        if self._index + 1 >= len(self._steps):
            self.done = True
        else:
            self._index += 1
